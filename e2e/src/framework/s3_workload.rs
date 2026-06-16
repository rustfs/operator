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
use aws_sdk_s3::{Client, config::Region, error::SdkError, primitives::ByteStream};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::time::timeout;

use crate::framework::history::{OperationKind, OperationOutcome, Recorder};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectSpec {
    pub key: String,
    pub size_bytes: usize,
    pub sha256: String,
    body: Vec<u8>,
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
    pub body: Option<Vec<u8>>,
}

impl ObjectSpec {
    pub fn deterministic(run_id: &str, index: usize, size_bytes: usize) -> Self {
        let key = format!("fault-e2e/{run_id}/object-{index:06}");
        let body = deterministic_bytes(index, size_bytes);
        let sha256 = sha256_hex(&body);

        Self {
            key,
            size_bytes,
            sha256,
            body,
        }
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
            "rustfs-e2e-static-credentials",
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

    pub async fn create_bucket(&self, recorder: &mut Recorder) -> Result<OperationOutcome> {
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
        object: &ObjectSpec,
        recorder: &mut Recorder,
    ) -> Result<OperationOutcome> {
        let record = recorder.begin(
            OperationKind::Put,
            self.bucket.clone(),
            Some(object.key.clone()),
            Some(object.sha256.clone()),
            Some(object.size_bytes),
        );
        let result = timeout(
            self.request_timeout,
            self.client
                .put_object()
                .bucket(&self.bucket)
                .key(&object.key)
                .body(ByteStream::from(object.body.clone()))
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
                    Some(format!("put object failed: {error}")),
                )?;
                Ok(outcome)
            }
            Err(_) => {
                recorder.finish(
                    record,
                    OperationOutcome::Timeout,
                    None,
                    Some("put object timed out".to_string()),
                )?;
                Ok(OperationOutcome::Timeout)
            }
        }
    }

    pub async fn get_object(&self, key: &str, recorder: &mut Recorder) -> Result<Option<Vec<u8>>> {
        Ok(self.get_object_result(key, recorder).await?.body)
    }

    pub async fn get_object_result(
        &self,
        key: &str,
        recorder: &mut Recorder,
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
                    body: Some(body),
                })
            }
            Ok(Err(error)) => {
                recorder.finish(
                    record,
                    OperationOutcome::Unknown,
                    Some(200),
                    Some(format!("get body read failed: {error}")),
                )?;
                Ok(GetObjectResult {
                    outcome: OperationOutcome::Unknown,
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
                    body: None,
                })
            }
        }
    }

    pub async fn head_object(
        &self,
        key: &str,
        recorder: &mut Recorder,
    ) -> Result<OperationOutcome> {
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
        recorder: &mut Recorder,
    ) -> Result<Option<Vec<String>>> {
        let record = recorder.begin(
            OperationKind::List,
            self.bucket.clone(),
            Some(prefix.to_string()),
            None,
            None,
        );
        let response = timeout(
            self.request_timeout,
            self.client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(prefix)
                .send(),
        )
        .await;

        match response {
            Ok(Ok(output)) => {
                let keys = output
                    .contents()
                    .iter()
                    .filter_map(|object| object.key().map(str::to_string))
                    .collect::<Vec<_>>();
                let mut record = record;
                record.size_bytes = Some(keys.len());
                recorder.finish(record, OperationOutcome::Ok, Some(200), None)?;
                Ok(Some(keys))
            }
            Ok(Err(error)) => {
                let outcome = classify_sdk_error(&error);
                recorder.finish(
                    record,
                    outcome,
                    sdk_error_status(&error),
                    Some(format!("list prefix failed: {error}")),
                )?;
                Ok(None)
            }
            Err(_) => {
                recorder.finish(
                    record,
                    OperationOutcome::Timeout,
                    None,
                    Some("list prefix timed out".to_string()),
                )?;
                Ok(None)
            }
        }
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

fn deterministic_bytes(index: usize, size_bytes: usize) -> Vec<u8> {
    (0..size_bytes)
        .map(|offset| ((offset + index * 31) % 251) as u8)
        .collect()
}

fn classify_sdk_error<E>(error: &SdkError<E>) -> OperationOutcome {
    match error {
        SdkError::TimeoutError(_) => OperationOutcome::Timeout,
        SdkError::DispatchFailure(_) | SdkError::ResponseError(_) => OperationOutcome::Unknown,
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
    use super::{ObjectSpec, sha256_hex};

    #[test]
    fn deterministic_objects_have_stable_keys_sizes_and_hashes() {
        let object = ObjectSpec::deterministic("run-1", 7, 4096);
        let same = ObjectSpec::deterministic("run-1", 7, 4096);

        assert_eq!(object.key, "fault-e2e/run-1/object-000007");
        assert_eq!(object.size_bytes, 4096);
        assert_eq!(object.sha256, same.sha256);
        assert_eq!(object.sha256, sha256_hex(&same.body));
    }
}
