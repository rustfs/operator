// Copyright 2025 RustFS Team
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use anyhow::{Context, Result};
use aws_config::BehaviorVersion;
use aws_credential_types::Credentials;
use aws_sdk_s3::{
    Client,
    config::Region,
    error::SdkError,
    primitives::ByteStream,
    types::{CompletedMultipartUpload, CompletedPart},
};
use serde::{Deserialize, Deserializer, Serialize, de};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::time::timeout;

use crate::fault::history::{OperationKind, OperationOutcome, OperationRecord, Recorder};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectSpec {
    pub key: String,
    pub size_bytes: usize,
    pub sha256: String,
    seed: u64,
    index: usize,
}

#[derive(Debug)]
pub struct PreparedObject {
    pub spec: ObjectSpec,
    body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkloadSizeClass {
    pub size_bytes: usize,
    pub object_count: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkloadPlan {
    pub seed: u64,
    pub generator: String,
    pub object_count: usize,
    pub concurrency: usize,
    pub total_payload_bytes: u64,
    pub size_distribution: Vec<WorkloadSizeClass>,
    sizes: Vec<usize>,
}

#[derive(Debug, Deserialize)]
struct SerializedWorkloadPlan {
    seed: u64,
    generator: String,
    object_count: usize,
    concurrency: usize,
    total_payload_bytes: u64,
    size_distribution: Vec<WorkloadSizeClass>,
}

#[derive(Clone)]
pub struct S3WorkloadClient {
    client: Client,
    bucket: String,
    request_timeout: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GetObjectResult {
    pub outcome: OperationOutcome,
    pub http_status: Option<u16>,
    pub error: Option<String>,
    pub body: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedWriteResult {
    pub write_outcome: OperationOutcome,
    pub verify_get_outcome: Option<OperationOutcome>,
    pub verified: bool,
}

impl ObjectSpec {
    pub fn key_prefix(run_id: &str) -> String {
        format!("fault-test/{run_id}/")
    }

    pub fn matches_body(&self, body: &[u8]) -> bool {
        body.len() == self.size_bytes && sha256_hex(body) == self.sha256
    }

    pub fn prepare_seeded(
        run_id: &str,
        index: usize,
        size_bytes: usize,
        seed: u64,
    ) -> PreparedObject {
        let key = format!("{}object-{index:06}", Self::key_prefix(run_id));
        let body = seeded_bytes(seed, index, size_bytes);
        let sha256 = sha256_hex(&body);

        PreparedObject {
            spec: Self {
                key,
                size_bytes,
                sha256,
                seed,
                index,
            },
            body,
        }
    }

    pub fn prepare(&self) -> PreparedObject {
        let body = seeded_bytes(self.seed, self.index, self.size_bytes);
        debug_assert_eq!(sha256_hex(&body), self.sha256);
        PreparedObject {
            spec: self.clone(),
            body,
        }
    }

    pub fn prepare_overwrite(&self, variant: u64) -> PreparedObject {
        let seed = self.seed ^ variant.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let body = seeded_bytes(seed, self.index, self.size_bytes);
        let sha256 = sha256_hex(&body);
        PreparedObject {
            spec: Self {
                key: self.key.clone(),
                size_bytes: self.size_bytes,
                sha256,
                seed,
                index: self.index,
            },
            body,
        }
    }
}

impl WorkloadPlan {
    const GENERATOR: &'static str = "splitmix64-v1";

    pub fn seeded(seed: u64, object_count: usize, concurrency: usize) -> Self {
        const SIZE_CLASSES: &[(usize, usize)] = &[
            (4 * 1024, 85),
            (16 * 1024, 10),
            (8 * 1024 * 1024, 4),
            (16 * 1024 * 1024, 1),
        ];

        let mut sizes = Vec::with_capacity(object_count);
        let mut size_distribution = Vec::with_capacity(SIZE_CLASSES.len());
        let mut assigned = 0;
        for (position, (size_bytes, weight)) in SIZE_CLASSES.iter().copied().enumerate() {
            let count = if position + 1 == SIZE_CLASSES.len() {
                object_count.saturating_sub(assigned)
            } else {
                object_count.saturating_mul(weight) / 100
            };
            sizes.extend(std::iter::repeat_n(size_bytes, count));
            size_distribution.push(WorkloadSizeClass {
                size_bytes,
                object_count: count,
            });
            assigned += count;
        }

        shuffle_sizes(&mut sizes, seed);
        let total_payload_bytes = sizes.iter().map(|size| *size as u64).sum();
        Self {
            seed,
            generator: Self::GENERATOR.to_string(),
            object_count,
            concurrency,
            total_payload_bytes,
            size_distribution,
            sizes,
        }
    }

    pub fn size_at(&self, index: usize) -> usize {
        self.sizes[index]
    }

    fn from_serialized(raw: SerializedWorkloadPlan) -> std::result::Result<Self, String> {
        if raw.generator != Self::GENERATOR {
            return Err(format!("unsupported workload generator {}", raw.generator));
        }
        if !(1..=raw.object_count).contains(&raw.concurrency) {
            return Err(format!(
                "workload concurrency {} must be between 1 and object_count {}",
                raw.concurrency, raw.object_count
            ));
        }

        let distributed_objects =
            raw.size_distribution
                .iter()
                .try_fold(0usize, |total, class| {
                    total.checked_add(class.object_count).ok_or_else(|| {
                        "workload size_distribution object_count overflowed".to_string()
                    })
                })?;
        if distributed_objects != raw.object_count {
            return Err(format!(
                "workload size_distribution object_count {} does not match object_count {}",
                distributed_objects, raw.object_count
            ));
        }

        let distributed_payload = raw
            .size_distribution
            .iter()
            .try_fold(0u64, |total, class| {
                if class.size_bytes == 0 {
                    return Err("workload size class size_bytes must be positive".to_string());
                }
                let class_payload = (class.size_bytes as u64)
                    .checked_mul(class.object_count as u64)
                    .ok_or_else(|| "workload size_distribution payload overflowed".to_string())?;
                total.checked_add(class_payload).ok_or_else(|| {
                    "workload size_distribution total payload overflowed".to_string()
                })
            })?;
        if distributed_payload != raw.total_payload_bytes {
            return Err(format!(
                "workload size_distribution payload {} does not match total_payload_bytes {}",
                distributed_payload, raw.total_payload_bytes
            ));
        }

        let mut sizes = Vec::with_capacity(raw.object_count);
        for class in &raw.size_distribution {
            sizes.extend(std::iter::repeat_n(class.size_bytes, class.object_count));
        }
        shuffle_sizes(&mut sizes, raw.seed);

        Ok(Self {
            seed: raw.seed,
            generator: raw.generator,
            object_count: raw.object_count,
            concurrency: raw.concurrency,
            total_payload_bytes: raw.total_payload_bytes,
            size_distribution: raw.size_distribution,
            sizes,
        })
    }
}

impl<'de> Deserialize<'de> for WorkloadPlan {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = SerializedWorkloadPlan::deserialize(deserializer)?;
        Self::from_serialized(raw).map_err(de::Error::custom)
    }
}

impl S3WorkloadClient {
    pub async fn new(
        endpoint: impl Into<String>,
        bucket: impl Into<String>,
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
        request_timeout: Duration,
    ) -> Result<Self> {
        let credentials = Credentials::new(
            access_key.into(),
            secret_key.into(),
            None,
            None,
            "rustfs-fault-test-static-credentials",
        );
        let shared_config = aws_config::defaults(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(credentials)
            .endpoint_url(endpoint.into())
            .load()
            .await;
        let s3_config = aws_sdk_s3::config::Builder::from(&shared_config)
            .force_path_style(true)
            .build();

        Ok(Self {
            client: Client::from_conf(s3_config),
            bucket: bucket.into(),
            request_timeout,
        })
    }

    pub async fn create_bucket(&self, recorder: &Recorder) -> Result<OperationOutcome> {
        let record = recorder.begin(
            OperationKind::CreateBucket,
            self.bucket.clone(),
            None,
            None,
            None,
        );
        let result = timeout(
            self.request_timeout,
            self.client.create_bucket().bucket(&self.bucket).send(),
        )
        .await;

        match result {
            Ok(Ok(_)) => {
                recorder.finish(record, OperationOutcome::Ok, Some(200), None)?;
                Ok(OperationOutcome::Ok)
            }
            Ok(Err(error)) => {
                let outcome = classify_sdk_error(&error);
                recorder.finish(
                    record,
                    outcome,
                    sdk_error_status(&error),
                    Some(format!("create bucket failed: {error}")),
                )?;
                Ok(outcome)
            }
            Err(_) => {
                recorder.finish(
                    record,
                    OperationOutcome::Timeout,
                    None,
                    Some("create bucket timed out".to_string()),
                )?;
                Ok(OperationOutcome::Timeout)
            }
        }
    }

    pub async fn put_object(
        &self,
        object: &PreparedObject,
        recorder: &Recorder,
    ) -> Result<OperationOutcome> {
        Ok(self.put_object_record(object, recorder).await?.outcome)
    }

    pub async fn put_object_record(
        &self,
        object: &PreparedObject,
        recorder: &Recorder,
    ) -> Result<OperationRecord> {
        let spec = &object.spec;
        let record = recorder.begin(
            OperationKind::Put,
            self.bucket.clone(),
            Some(spec.key.clone()),
            Some(spec.sha256.clone()),
            Some(spec.size_bytes),
        );
        let result = timeout(
            self.request_timeout,
            self.client
                .put_object()
                .bucket(&self.bucket)
                .key(&spec.key)
                .body(ByteStream::from(object.body.clone()))
                .send(),
        )
        .await;

        match result {
            Ok(Ok(_)) => recorder.finish(record, OperationOutcome::Ok, Some(200), None),
            Ok(Err(error)) => {
                let outcome = classify_sdk_error(&error);
                recorder.finish(
                    record,
                    outcome,
                    sdk_error_status(&error),
                    Some(format!("put object failed: {error}")),
                )
            }
            Err(_) => recorder.finish(
                record,
                OperationOutcome::Timeout,
                None,
                Some("put object timed out".to_string()),
            ),
        }
    }

    pub async fn get_object(&self, key: &str, recorder: &Recorder) -> Result<Option<Vec<u8>>> {
        Ok(self.get_object_result(key, recorder).await?.body)
    }

    pub async fn get_object_result(
        &self,
        key: &str,
        recorder: &Recorder,
    ) -> Result<GetObjectResult> {
        let record = recorder.begin(
            OperationKind::Get,
            self.bucket.clone(),
            Some(key.to_string()),
            None,
            None,
        );
        let response = timeout(
            self.request_timeout,
            self.client
                .get_object()
                .bucket(&self.bucket)
                .key(key)
                .send(),
        )
        .await;

        let output = match response {
            Ok(Ok(output)) => output,
            Ok(Err(error)) => {
                let outcome = classify_sdk_error(&error);
                recorder.finish(
                    record,
                    outcome,
                    sdk_error_status(&error),
                    Some(format!("get object failed: {error}")),
                )?;
                return Ok(GetObjectResult {
                    outcome,
                    http_status: sdk_error_status(&error),
                    error: Some(format!("get object failed: {error}")),
                    body: None,
                });
            }
            Err(_) => {
                recorder.finish(
                    record,
                    OperationOutcome::Timeout,
                    None,
                    Some("get object timed out".to_string()),
                )?;
                return Ok(GetObjectResult {
                    outcome: OperationOutcome::Timeout,
                    http_status: None,
                    error: Some("get object timed out".to_string()),
                    body: None,
                });
            }
        };

        let body = timeout(self.request_timeout, output.body.collect()).await;
        match body {
            Ok(Ok(bytes)) => {
                let body = bytes.into_bytes().to_vec();
                let mut record = record;
                record.value_sha256 = Some(sha256_hex(&body));
                record.size_bytes = Some(body.len());
                recorder.finish(record, OperationOutcome::Ok, Some(200), None)?;
                Ok(GetObjectResult {
                    outcome: OperationOutcome::Ok,
                    http_status: Some(200),
                    error: None,
                    body: Some(body),
                })
            }
            Ok(Err(error)) => {
                let error = format!("get body read failed: {error}");
                recorder.finish(
                    record,
                    OperationOutcome::Unknown,
                    Some(200),
                    Some(error.clone()),
                )?;
                Ok(GetObjectResult {
                    outcome: OperationOutcome::Unknown,
                    http_status: Some(200),
                    error: Some(error),
                    body: None,
                })
            }
            Err(_) => {
                recorder.finish(
                    record,
                    OperationOutcome::Timeout,
                    Some(200),
                    Some("get body read timed out".to_string()),
                )?;
                Ok(GetObjectResult {
                    outcome: OperationOutcome::Timeout,
                    http_status: Some(200),
                    error: Some("get body read timed out".to_string()),
                    body: None,
                })
            }
        }
    }

    pub async fn put_and_verify_object(
        &self,
        object: &PreparedObject,
        recorder: &Recorder,
    ) -> Result<VerifiedWriteResult> {
        let write_outcome = self.put_object(object, recorder).await?;
        if write_outcome != OperationOutcome::Ok {
            return Ok(VerifiedWriteResult {
                write_outcome,
                verify_get_outcome: None,
                verified: false,
            });
        }

        let get = self.get_object_result(&object.spec.key, recorder).await?;
        let verified = get
            .body
            .as_deref()
            .is_some_and(|body| object.spec.matches_body(body));
        Ok(VerifiedWriteResult {
            write_outcome,
            verify_get_outcome: Some(get.outcome),
            verified,
        })
    }

    pub async fn delete_object(&self, key: &str, recorder: &Recorder) -> Result<OperationOutcome> {
        let record = recorder.begin(
            OperationKind::Delete,
            self.bucket.clone(),
            Some(key.to_string()),
            None,
            None,
        );
        let result = timeout(
            self.request_timeout,
            self.client
                .delete_object()
                .bucket(&self.bucket)
                .key(key)
                .send(),
        )
        .await;

        match result {
            Ok(Ok(_)) => {
                recorder.finish(record, OperationOutcome::Ok, Some(204), None)?;
                Ok(OperationOutcome::Ok)
            }
            Ok(Err(error)) => {
                let outcome = classify_sdk_error(&error);
                recorder.finish(
                    record,
                    outcome,
                    sdk_error_status(&error),
                    Some(format!("delete object failed: {error}")),
                )?;
                Ok(outcome)
            }
            Err(_) => {
                recorder.finish(
                    record,
                    OperationOutcome::Timeout,
                    None,
                    Some("delete object timed out".to_string()),
                )?;
                Ok(OperationOutcome::Timeout)
            }
        }
    }

    pub async fn delete_and_verify_absent(
        &self,
        key: &str,
        recorder: &Recorder,
    ) -> Result<(OperationOutcome, Option<OperationOutcome>)> {
        let delete_outcome = self.delete_object(key, recorder).await?;
        if delete_outcome != OperationOutcome::Ok {
            return Ok((delete_outcome, None));
        }
        let get = self.get_object_result(key, recorder).await?;
        Ok((delete_outcome, Some(get.outcome)))
    }

    pub async fn complete_multipart_object(
        &self,
        object: &PreparedObject,
        recorder: &Recorder,
    ) -> Result<OperationOutcome> {
        let Some(upload_id) = self
            .create_multipart_upload(&object.spec.key, recorder)
            .await?
        else {
            return Ok(OperationOutcome::Unknown);
        };
        let mut completed_parts = Vec::new();
        for (index, chunk) in object.body.chunks(5 * 1024 * 1024).enumerate() {
            let part_number = (index + 1) as i32;
            match self
                .upload_part(&object.spec.key, &upload_id, part_number, chunk, recorder)
                .await?
            {
                Some(part) => completed_parts.push(part),
                None => {
                    let _ = self
                        .abort_multipart_upload(&object.spec.key, &upload_id, recorder)
                        .await;
                    return Ok(OperationOutcome::Unknown);
                }
            }
        }

        let record = recorder.begin(
            OperationKind::CompleteMultipartUpload,
            self.bucket.clone(),
            Some(object.spec.key.clone()),
            Some(object.spec.sha256.clone()),
            Some(object.spec.size_bytes),
        );
        let upload = CompletedMultipartUpload::builder()
            .set_parts(Some(completed_parts))
            .build();
        let result = timeout(
            self.request_timeout,
            self.client
                .complete_multipart_upload()
                .bucket(&self.bucket)
                .key(&object.spec.key)
                .upload_id(upload_id)
                .multipart_upload(upload)
                .send(),
        )
        .await;

        match result {
            Ok(Ok(_)) => {
                recorder.finish(record, OperationOutcome::Ok, Some(200), None)?;
                Ok(OperationOutcome::Ok)
            }
            Ok(Err(error)) => {
                let outcome = classify_sdk_error(&error);
                recorder.finish(
                    record,
                    outcome,
                    sdk_error_status(&error),
                    Some(format!("complete multipart upload failed: {error}")),
                )?;
                Ok(outcome)
            }
            Err(_) => {
                recorder.finish(
                    record,
                    OperationOutcome::Timeout,
                    None,
                    Some("complete multipart upload timed out".to_string()),
                )?;
                Ok(OperationOutcome::Timeout)
            }
        }
    }

    pub async fn abort_multipart_object(
        &self,
        object: &PreparedObject,
        recorder: &Recorder,
    ) -> Result<OperationOutcome> {
        let Some(upload_id) = self
            .create_multipart_upload(&object.spec.key, recorder)
            .await?
        else {
            return Ok(OperationOutcome::Unknown);
        };
        self.abort_multipart_upload(&object.spec.key, &upload_id, recorder)
            .await
    }

    async fn create_multipart_upload(
        &self,
        key: &str,
        recorder: &Recorder,
    ) -> Result<Option<String>> {
        let record = recorder.begin(
            OperationKind::CreateMultipartUpload,
            self.bucket.clone(),
            Some(key.to_string()),
            None,
            None,
        );
        let result = timeout(
            self.request_timeout,
            self.client
                .create_multipart_upload()
                .bucket(&self.bucket)
                .key(key)
                .send(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                let Some(upload_id) = output.upload_id().map(str::to_string) else {
                    recorder.finish(
                        record,
                        OperationOutcome::Unknown,
                        Some(200),
                        Some("create multipart upload omitted upload_id".to_string()),
                    )?;
                    return Ok(None);
                };
                recorder.finish(record, OperationOutcome::Ok, Some(200), None)?;
                Ok(Some(upload_id))
            }
            Ok(Err(error)) => {
                let outcome = classify_sdk_error(&error);
                recorder.finish(
                    record,
                    outcome,
                    sdk_error_status(&error),
                    Some(format!("create multipart upload failed: {error}")),
                )?;
                Ok(None)
            }
            Err(_) => {
                recorder.finish(
                    record,
                    OperationOutcome::Timeout,
                    None,
                    Some("create multipart upload timed out".to_string()),
                )?;
                Ok(None)
            }
        }
    }

    async fn upload_part(
        &self,
        key: &str,
        upload_id: &str,
        part_number: i32,
        body: &[u8],
        recorder: &Recorder,
    ) -> Result<Option<CompletedPart>> {
        let record = recorder.begin(
            OperationKind::UploadPart,
            self.bucket.clone(),
            Some(key.to_string()),
            Some(sha256_hex(body)),
            Some(body.len()),
        );
        let result = timeout(
            self.request_timeout,
            self.client
                .upload_part()
                .bucket(&self.bucket)
                .key(key)
                .upload_id(upload_id)
                .part_number(part_number)
                .body(ByteStream::from(body.to_vec()))
                .send(),
        )
        .await;

        match result {
            Ok(Ok(output)) => {
                let Some(e_tag) = output.e_tag().map(str::to_string) else {
                    recorder.finish(
                        record,
                        OperationOutcome::Unknown,
                        Some(200),
                        Some(format!("upload part {part_number} omitted ETag")),
                    )?;
                    return Ok(None);
                };
                recorder.finish(record, OperationOutcome::Ok, Some(200), None)?;
                Ok(Some(
                    CompletedPart::builder()
                        .part_number(part_number)
                        .e_tag(e_tag)
                        .build(),
                ))
            }
            Ok(Err(error)) => {
                let outcome = classify_sdk_error(&error);
                recorder.finish(
                    record,
                    outcome,
                    sdk_error_status(&error),
                    Some(format!("upload part {part_number} failed: {error}")),
                )?;
                Ok(None)
            }
            Err(_) => {
                recorder.finish(
                    record,
                    OperationOutcome::Timeout,
                    None,
                    Some(format!("upload part {part_number} timed out")),
                )?;
                Ok(None)
            }
        }
    }

    async fn abort_multipart_upload(
        &self,
        key: &str,
        upload_id: &str,
        recorder: &Recorder,
    ) -> Result<OperationOutcome> {
        let record = recorder.begin(
            OperationKind::AbortMultipartUpload,
            self.bucket.clone(),
            Some(key.to_string()),
            None,
            None,
        );
        let result = timeout(
            self.request_timeout,
            self.client
                .abort_multipart_upload()
                .bucket(&self.bucket)
                .key(key)
                .upload_id(upload_id)
                .send(),
        )
        .await;

        match result {
            Ok(Ok(_)) => {
                recorder.finish(record, OperationOutcome::Ok, Some(204), None)?;
                Ok(OperationOutcome::Ok)
            }
            Ok(Err(error)) => {
                let outcome = classify_sdk_error(&error);
                recorder.finish(
                    record,
                    outcome,
                    sdk_error_status(&error),
                    Some(format!("abort multipart upload failed: {error}")),
                )?;
                Ok(outcome)
            }
            Err(_) => {
                recorder.finish(
                    record,
                    OperationOutcome::Timeout,
                    None,
                    Some("abort multipart upload timed out".to_string()),
                )?;
                Ok(OperationOutcome::Timeout)
            }
        }
    }

    pub async fn head_object(&self, key: &str, recorder: &Recorder) -> Result<OperationOutcome> {
        let record = recorder.begin(
            OperationKind::Head,
            self.bucket.clone(),
            Some(key.to_string()),
            None,
            None,
        );
        let result = timeout(
            self.request_timeout,
            self.client
                .head_object()
                .bucket(&self.bucket)
                .key(key)
                .send(),
        )
        .await;

        match result {
            Ok(Ok(_)) => {
                recorder.finish(record, OperationOutcome::Ok, Some(200), None)?;
                Ok(OperationOutcome::Ok)
            }
            Ok(Err(error)) => {
                let outcome = classify_sdk_error(&error);
                recorder.finish(
                    record,
                    outcome,
                    sdk_error_status(&error),
                    Some(format!("head object failed: {error}")),
                )?;
                Ok(outcome)
            }
            Err(_) => {
                recorder.finish(
                    record,
                    OperationOutcome::Timeout,
                    None,
                    Some("head object timed out".to_string()),
                )?;
                Ok(OperationOutcome::Timeout)
            }
        }
    }

    pub async fn list_prefix(
        &self,
        prefix: &str,
        recorder: &Recorder,
    ) -> Result<Option<Vec<String>>> {
        let record = recorder.begin(
            OperationKind::List,
            self.bucket.clone(),
            Some(prefix.to_string()),
            None,
            None,
        );
        let mut keys = Vec::new();
        let mut continuation_token = None;
        loop {
            let mut request = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(prefix);
            if let Some(token) = continuation_token.as_deref() {
                request = request.continuation_token(token);
            }
            let response = timeout(self.request_timeout, request.send()).await;
            let output = match response {
                Ok(Ok(output)) => output,
                Ok(Err(error)) => {
                    let outcome = classify_sdk_error(&error);
                    recorder.finish(
                        record,
                        outcome,
                        sdk_error_status(&error),
                        Some(format!("list prefix failed: {error}")),
                    )?;
                    return Ok(None);
                }
                Err(_) => {
                    recorder.finish(
                        record,
                        OperationOutcome::Timeout,
                        None,
                        Some("list prefix timed out".to_string()),
                    )?;
                    return Ok(None);
                }
            };
            keys.extend(
                output
                    .contents()
                    .iter()
                    .filter_map(|object| object.key().map(str::to_string)),
            );
            if !output.is_truncated().unwrap_or(false) {
                break;
            }
            continuation_token = output.next_continuation_token().map(str::to_string);
            if continuation_token.is_none() {
                recorder.finish(
                    record,
                    OperationOutcome::Unknown,
                    Some(200),
                    Some("truncated LIST response omitted continuation token".to_string()),
                )?;
                return Ok(None);
            }
        }

        let mut record = record;
        record.size_bytes = Some(keys.len());
        record.listed_keys = Some(keys.clone());
        recorder.finish(record, OperationOutcome::Ok, Some(200), None)?;
        Ok(Some(keys))
    }
}

pub fn sha256_hex(body: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(body);
    hex::encode(hasher.finalize())
}

pub async fn wait_for_s3_endpoint(endpoint: &str, timeout_duration: Duration) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .context("build S3 readiness HTTP client")?;
    let start = std::time::Instant::now();

    loop {
        if client.get(endpoint).send().await.is_ok() {
            return Ok(());
        }
        if start.elapsed() >= timeout_duration {
            anyhow::bail!("timed out waiting for S3 endpoint {endpoint}");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

fn seeded_bytes(seed: u64, index: usize, size_bytes: usize) -> Vec<u8> {
    let mut generator = SplitMix64::new(seed ^ (index as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93));
    let mut body = vec![0; size_bytes];
    for chunk in body.chunks_mut(8) {
        let bytes = generator.next_u64().to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
    body
}

fn shuffle_sizes(sizes: &mut [usize], seed: u64) {
    let mut generator = SplitMix64::new(seed ^ 0xA076_1D64_78BD_642F);
    for index in (1..sizes.len()).rev() {
        let swap_with = (generator.next_u64() % (index as u64 + 1)) as usize;
        sizes.swap(index, swap_with);
    }
}

struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        value ^ (value >> 31)
    }
}

fn classify_sdk_error<E>(error: &SdkError<E>) -> OperationOutcome {
    match error {
        SdkError::TimeoutError(_) => OperationOutcome::Timeout,
        SdkError::DispatchFailure(_) | SdkError::ResponseError(_) => OperationOutcome::Unknown,
        SdkError::ServiceError(context) if context.raw().status().as_u16() == 404 => {
            OperationOutcome::NotFound
        }
        SdkError::ConstructionFailure(_) | SdkError::ServiceError(_) => OperationOutcome::Failed,
        _ => OperationOutcome::Unknown,
    }
}

fn sdk_error_status<E>(error: &SdkError<E>) -> Option<u16> {
    match error {
        SdkError::ServiceError(context) => Some(context.raw().status().as_u16()),
        SdkError::ResponseError(context) => Some(context.raw().status().as_u16()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{ObjectSpec, WorkloadPlan, sha256_hex};

    #[test]
    fn seeded_objects_have_stable_keys_sizes_and_hashes() {
        let object = ObjectSpec::prepare_seeded("run-1", 7, 4096, 42);
        let same = ObjectSpec::prepare_seeded("run-1", 7, 4096, 42);

        assert_eq!(ObjectSpec::key_prefix("run-1"), "fault-test/run-1/");
        assert_eq!(object.spec.key, "fault-test/run-1/object-000007");
        assert_eq!(object.spec.size_bytes, 4096);
        assert_eq!(object.spec.sha256, same.spec.sha256);
        assert_eq!(object.spec.sha256, sha256_hex(&same.body));
        assert!(object.spec.matches_body(&same.body));
        assert_ne!(
            object.spec.sha256,
            ObjectSpec::prepare_seeded("run-1", 7, 4096, 43).spec.sha256
        );

        let mut corrupted = same.body.clone();
        corrupted[0] ^= 1;
        assert!(!object.spec.matches_body(&corrupted));
        assert!(!object.spec.matches_body(&same.body[..same.body.len() - 1]));
    }

    #[test]
    fn workload_plan_is_weighted_shuffled_and_reproducible() {
        let plan = WorkloadPlan::seeded(42, 40000, 80);
        let same = WorkloadPlan::seeded(42, 40000, 80);
        let different = WorkloadPlan::seeded(43, 40000, 80);

        assert_eq!(plan, same);
        assert_ne!(plan.sizes, different.sizes);
        assert_eq!(
            plan.size_distribution
                .iter()
                .map(|class| (class.size_bytes, class.object_count))
                .collect::<Vec<_>>(),
            vec![
                (4 * 1024, 34000),
                (16 * 1024, 4000),
                (8 * 1024 * 1024, 1600),
                (16 * 1024 * 1024, 400),
            ]
        );
        assert_eq!(plan.total_payload_bytes, 20_337_459_200);
        assert_eq!(plan.concurrency, 80);
    }

    #[test]
    fn workload_plan_deserialization_rehydrates_runtime_sizes() {
        let plan = WorkloadPlan::seeded(42, 128, 8);
        let encoded = serde_json::to_string(&plan).expect("workload plan json");
        let decoded =
            serde_json::from_str::<WorkloadPlan>(&encoded).expect("decoded workload plan");

        assert_eq!(decoded, plan);
        assert_eq!(decoded.size_at(0), plan.size_at(0));
        assert_eq!(decoded.size_at(127), plan.size_at(127));
    }

    #[test]
    fn workload_plan_deserialization_rejects_invalid_distribution() {
        let plan = WorkloadPlan::seeded(42, 128, 8);
        let mut value = serde_json::to_value(&plan).expect("workload plan json");
        value["object_count"] = serde_json::json!(129);

        let result = serde_json::from_value::<WorkloadPlan>(value);

        assert!(result.is_err());
    }
}
