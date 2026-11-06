// Copyright 2026 RustFS Team
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

use super::statefulset_owned_by_tenant;
use crate::context::{Context, Error};
use crate::types;
use crate::types::v1alpha1::tenant::Tenant;
use k8s_openapi::api::apps::v1::StatefulSet;
use kube::ResourceExt;
use serde_json::json;

/// Transfers timing ownership before the ordinary apply releases those fields.
/// UID and resourceVersion preconditions protect both requests from replacement
/// or concurrent changes to the observed StatefulSet.
pub(super) async fn apply_statefulset(
    ctx: &Context,
    tenant: &Tenant,
    namespace: &str,
    existing: &StatefulSet,
    mut desired: StatefulSet,
) -> Result<StatefulSet, Error> {
    if !statefulset_owned_by_tenant(existing, tenant) {
        return Err(types::error::Error::InternalError {
            msg: format!(
                "StatefulSet '{}' is not owned by Tenant '{}'",
                existing.name_any(),
                tenant.name()
            ),
        }
        .into());
    }
    let uid = existing
        .metadata
        .uid
        .as_ref()
        .ok_or_else(|| types::error::Error::InternalError {
            msg: format!("StatefulSet '{}' has no UID", existing.name_any()),
        })?;
    let resource_version = existing.metadata.resource_version.as_ref().ok_or_else(|| {
        types::error::Error::InternalError {
            msg: format!(
                "StatefulSet '{}' has no resourceVersion",
                existing.name_any()
            ),
        }
    })?;
    desired.metadata.uid = Some(uid.clone());
    desired.metadata.resource_version = Some(resource_version.clone());

    let existing_probe = existing
        .spec
        .as_ref()
        .and_then(|spec| spec.template.spec.as_ref())
        .and_then(|spec| {
            spec.containers
                .iter()
                .find(|container| container.name == "rustfs")
        })
        .and_then(|container| container.readiness_probe.as_ref());
    // Kubernetes requires a handler when adding a probe to an existing container.
    if existing_probe.is_none() {
        return ctx.apply(&desired, namespace).await;
    }

    let probe = desired
        .spec
        .as_mut()
        .and_then(|spec| spec.template.spec.as_mut())
        .and_then(|spec| {
            spec.containers
                .iter_mut()
                .find(|container| container.name == "rustfs")
        })
        .and_then(|container| container.readiness_probe.as_mut())
        .ok_or_else(|| types::error::Error::InternalError {
            msg: "generated StatefulSet has no RustFS readiness probe".to_string(),
        })?;
    let patch = json!({
        "apiVersion": "apps/v1",
        "kind": "StatefulSet",
        "metadata": {
            "name": existing.name_any(),
            "namespace": namespace,
            "uid": uid,
            "resourceVersion": resource_version,
        },
        "spec": {
            "template": {
                "spec": {
                    "containers": [{
                        "name": "rustfs",
                        "readinessProbe": {
                            "initialDelaySeconds": probe.initial_delay_seconds,
                            "timeoutSeconds": probe.timeout_seconds,
                            "periodSeconds": probe.period_seconds,
                            "failureThreshold": probe.failure_threshold,
                            "successThreshold": probe.success_threshold,
                        }
                    }]
                }
            }
        }
    });
    let updated = ctx
        .apply_readiness_fields(&existing.name_any(), namespace, &patch)
        .await?;

    // The dedicated manager retains the timing values when the general manager
    // releases them, including fields previously changed with kubectl patch.
    probe.initial_delay_seconds = None;
    probe.timeout_seconds = None;
    probe.period_seconds = None;
    probe.failure_threshold = None;
    probe.success_threshold = None;
    desired.metadata.resource_version = updated.metadata.resource_version;
    ctx.apply(&desired, namespace).await
}

#[cfg(test)]
#[path = "readiness_tests.rs"]
mod tests;
