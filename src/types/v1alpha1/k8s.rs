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

//! Common Kubernetes enum types used across the operator

use k8s_openapi::schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use strum::Display;

/// Pod management policy for StatefulSets
/// - OrderedReady: Respect the ordering guarantees demonstrated
/// - Parallel: launch or terminate all Pods in parallel, and not to wait for Pods to become Running
///   and Ready or completely terminated prior to launching or terminating another Pod
///
///https://kubernetes.io/docs/tutorials/stateful-application/basic-stateful-set/#pod-management-policy
#[derive(Default, Deserialize, Serialize, Clone, Debug, JsonSchema, Display)]
#[serde(rename_all = "PascalCase")]
#[schemars(rename_all = "PascalCase")]
pub enum PodManagementPolicy {
    #[strum(to_string = "OrderedReady")]
    OrderedReady,

    #[strum(to_string = "Parallel")]
    #[default]
    Parallel,
}

/// Image pull policy for containers.
/// - Always: Always pull the image
/// - Never: Never pull the image
/// - IfNotPresent: Pull the image if not present locally (default)
///
/// https://kubernetes.io/docs/concepts/containers/images/#image-pull-policy
#[derive(Default, Deserialize, Serialize, Clone, Debug, JsonSchema, Display)]
#[serde(rename_all = "PascalCase")]
#[schemars(rename_all = "PascalCase")]
pub enum ImagePullPolicy {
    #[strum(to_string = "Always")]
    Always,

    #[strum(to_string = "Never")]
    Never,

    #[strum(to_string = "IfNotPresent")]
    #[default]
    IfNotPresent,
}

/// Pod deletion policy when the node hosting the Pod is down (NotReady/Unknown).
///
/// This is primarily intended to unblock StatefulSet pods stuck in terminating state
/// when the node becomes unreachable.
///
/// WARNING: Force-deleting pods can have data consistency implications depending on
/// your storage backend and workload semantics.
#[derive(Default, Deserialize, Serialize, Clone, Debug, JsonSchema, Display, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
#[schemars(rename_all = "PascalCase")]
pub enum PodDeletionPolicyWhenNodeIsDown {
    /// Do not delete pods automatically.
    #[strum(to_string = "DoNothing")]
    #[default]
    DoNothing,

    /// Request a normal delete for the pod.
    #[strum(to_string = "Delete")]
    Delete,

    /// Force delete the pod with gracePeriodSeconds=0.
    #[strum(to_string = "ForceDelete")]
    ForceDelete,

    /// Longhorn-compatible: force delete StatefulSet terminating pods on down nodes.
    #[strum(to_string = "DeleteStatefulSetPod")]
    DeleteStatefulSetPod,

    /// Longhorn-compatible: force delete Deployment terminating pods on down nodes.
    /// (Deployment pods are owned by ReplicaSet.)
    #[strum(to_string = "DeleteDeploymentPod")]
    DeleteDeploymentPod,

    /// Longhorn-compatible: force delete both StatefulSet and Deployment terminating pods on down nodes.
    #[strum(to_string = "DeleteBothStatefulSetAndDeploymentPod")]
    DeleteBothStatefulSetAndDeploymentPod,
}
