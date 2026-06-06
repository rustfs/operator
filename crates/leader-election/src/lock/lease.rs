// Copyright 2024 RustFS Team
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

//! Kubernetes Lease-based lock implementation.

use async_trait::async_trait;
use k8s_openapi::api::coordination::v1::{Lease, LeaseSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{MicroTime, ObjectMeta};
use kube::Client;
use kube::api::{Api, PostParams};
use tracing::debug;

use super::Lock;
use crate::error::Error;
use crate::record::LeaderElectionRecord;

/// A lock backed by a Kubernetes Lease resource.
pub struct LeaseLock {
    client: Client,
    name: String,
    namespace: String,
    identity: String,
    /// Cached lease object (for resourceVersion tracking).
    cached: tokio::sync::Mutex<Option<Lease>>,
}

impl LeaseLock {
    /// Create a new LeaseLock.
    pub fn new(
        client: Client,
        name: impl Into<String>,
        namespace: impl Into<String>,
        identity: impl Into<String>,
    ) -> Self {
        Self {
            client,
            name: name.into(),
            namespace: namespace.into(),
            identity: identity.into(),
            cached: tokio::sync::Mutex::new(None),
        }
    }

    /// Build a Lease object from a LeaderElectionRecord.
    fn build_lease(
        &self,
        record: &LeaderElectionRecord,
        resource_version: Option<String>,
    ) -> Lease {
        Lease {
            metadata: ObjectMeta {
                name: Some(self.name.clone()),
                namespace: Some(self.namespace.clone()),
                resource_version,
                ..Default::default()
            },
            spec: Some(record_to_spec(record)),
        }
    }

    /// Return a namespaced Api handle for Lease resources.
    fn api(&self) -> Api<Lease> {
        Api::<Lease>::namespaced(self.client.clone(), &self.namespace)
    }
}

/// Convert a LeaderElectionRecord into a LeaseSpec.
fn record_to_spec(record: &LeaderElectionRecord) -> LeaseSpec {
    LeaseSpec {
        holder_identity: Some(record.holder_identity.clone()),
        lease_duration_seconds: Some(record.lease_duration_seconds),
        acquire_time: Some(MicroTime(record.acquire_time)),
        renew_time: Some(MicroTime(record.renew_time)),
        lease_transitions: Some(record.leader_transitions),
    }
}

/// Convert a LeaseSpec into a LeaderElectionRecord.
///
/// Returns `None` if required fields are missing from the spec.
fn spec_to_record(spec: &LeaseSpec) -> Option<LeaderElectionRecord> {
    let acquire_time = spec.acquire_time.as_ref()?.0;
    let renew_time = spec.renew_time.as_ref()?.0;

    Some(LeaderElectionRecord {
        holder_identity: spec.holder_identity.clone().unwrap_or_default(),
        lease_duration_seconds: spec.lease_duration_seconds.unwrap_or(0),
        acquire_time,
        renew_time,
        leader_transitions: spec.lease_transitions.unwrap_or(0),
    })
}

/// Check whether a kube::Error represents a 409 Conflict.
fn is_conflict(err: &kube::Error) -> bool {
    matches!(err, kube::Error::Api(e) if e.code == 409)
}

#[async_trait]
impl Lock for LeaseLock {
    async fn get(&self) -> Result<Option<LeaderElectionRecord>, Error> {
        let api = self.api();

        // get_opt returns Ok(None) on 404, propagating other errors.
        let lease = match api.get_opt(&self.name).await {
            Ok(Some(lease)) => lease,
            Ok(None) => return Ok(None),
            Err(e) => return Err(Error::KubeApi { source: e }),
        };

        let record = lease.spec.as_ref().and_then(spec_to_record);

        // Cache the lease for subsequent update() calls (preserves resourceVersion).
        {
            let mut cached = self.cached.lock().await;
            *cached = Some(lease);
        }

        debug!(
            lease = %self.describe(),
            has_record = record.is_some(),
            "fetched lease"
        );

        Ok(record)
    }

    async fn create(&self, record: LeaderElectionRecord) -> Result<(), Error> {
        let api = self.api();
        let lease = self.build_lease(&record, None);

        let created = api
            .create(&PostParams::default(), &lease)
            .await
            .map_err(|e| Error::KubeApi { source: e })?;

        // Cache the created lease (server assigns resourceVersion).
        {
            let mut cached = self.cached.lock().await;
            *cached = Some(created);
        }

        debug!(
            lease = %self.describe(),
            holder = %record.holder_identity,
            "created lease"
        );

        Ok(())
    }

    async fn update(&self, record: LeaderElectionRecord) -> Result<(), Error> {
        // Retrieve the cached resourceVersion from the last get/create/update.
        let resource_version = {
            let cached = self.cached.lock().await;
            cached
                .as_ref()
                .and_then(|l| l.metadata.resource_version.clone())
        };

        let resource_version = resource_version.ok_or_else(|| Error::Conflict)?;

        let api = self.api();
        let lease = self.build_lease(&record, Some(resource_version));

        let updated = api
            .replace(&self.name, &PostParams::default(), &lease)
            .await
            .map_err(|e| {
                if is_conflict(&e) {
                    Error::Conflict
                } else {
                    Error::KubeApi { source: e }
                }
            })?;

        // Update cache with the new resourceVersion.
        {
            let mut cached = self.cached.lock().await;
            *cached = Some(updated);
        }

        debug!(
            lease = %self.describe(),
            holder = %record.holder_identity,
            "updated lease"
        );

        Ok(())
    }

    fn identity(&self) -> &str {
        &self.identity
    }

    fn describe(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn sample_record() -> LeaderElectionRecord {
        LeaderElectionRecord {
            holder_identity: "pod-abc".into(),
            lease_duration_seconds: 15,
            acquire_time: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            renew_time: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 10).unwrap(),
            leader_transitions: 3,
        }
    }

    #[test]
    fn record_to_spec_roundtrip() {
        let record = sample_record();
        let spec = record_to_spec(&record);
        let back = spec_to_record(&spec).expect("roundtrip should succeed");

        assert_eq!(back.holder_identity, record.holder_identity);
        assert_eq!(back.lease_duration_seconds, record.lease_duration_seconds);
        assert_eq!(back.acquire_time, record.acquire_time);
        assert_eq!(back.renew_time, record.renew_time);
        assert_eq!(back.leader_transitions, record.leader_transitions);
    }

    #[test]
    fn spec_to_record_empty_holder() {
        let spec = LeaseSpec {
            holder_identity: None,
            lease_duration_seconds: Some(15),
            acquire_time: Some(MicroTime(Utc::now())),
            renew_time: Some(MicroTime(Utc::now())),
            lease_transitions: Some(0),
        };
        let record = spec_to_record(&spec).unwrap();
        assert_eq!(record.holder_identity, "");
    }

    #[test]
    fn spec_to_record_missing_times() {
        let spec = LeaseSpec {
            holder_identity: Some("pod-1".into()),
            lease_duration_seconds: Some(15),
            acquire_time: None,
            renew_time: None,
            lease_transitions: None,
        };
        assert!(spec_to_record(&spec).is_none());
    }
}
