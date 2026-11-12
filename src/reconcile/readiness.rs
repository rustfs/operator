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
use k8s_openapi::apimachinery::pkg::apis::meta::v1::StatusCause;
use kube::ResourceExt;
use kube::api::PatchParams;

/// Applies the complete Pod template atomically, reclaiming timing fields only
/// when Kubernetes reports that every conflict belongs to those fields.
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

    let params = PatchParams::apply("rustfs-operator");
    match ctx.apply_statefulset(&desired, namespace, &params).await {
        Ok(updated) => Ok(updated),
        Err(error) if only_readiness_timing_conflicts(&error.causes) => {
            // The rejected apply persisted nothing. Reuse its complete template
            // and preconditions so concurrent changes cannot broaden ownership.
            ctx.apply_statefulset(&desired, namespace, &params.force())
                .await
                .map_err(|error| Error::Kube {
                    source: *error.source,
                })
        }
        Err(error) => Err(Error::Kube {
            source: *error.source,
        }),
    }
}

fn only_readiness_timing_conflicts(causes: &[StatusCause]) -> bool {
    !causes.is_empty()
        && causes.iter().all(|cause| {
            cause.reason.as_deref() == Some("FieldManagerConflict")
                && matches!(
                    cause.field.as_deref(),
                    Some(
                        ".spec.template.spec.containers[name=\"rustfs\"].readinessProbe.initialDelaySeconds"
                            | ".spec.template.spec.containers[name=\"rustfs\"].readinessProbe.timeoutSeconds"
                            | ".spec.template.spec.containers[name=\"rustfs\"].readinessProbe.periodSeconds"
                            | ".spec.template.spec.containers[name=\"rustfs\"].readinessProbe.failureThreshold"
                            | ".spec.template.spec.containers[name=\"rustfs\"].readinessProbe.successThreshold"
                    )
                )
        })
}

#[cfg(test)]
#[path = "readiness_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "readiness_rollout_tests.rs"]
mod rollout_tests;
