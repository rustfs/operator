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

/// Pod management policy for StatefulSets
#[derive(Default, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "PascalCase")]
#[schemars(rename_all = "PascalCase")]
pub enum PodManagementPolicy {
    OrderedReady,

    #[default]
    Parallel,
}

/// Image pull policy for containers
#[derive(Default, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[serde(rename_all = "PascalCase")]
#[schemars(rename_all = "PascalCase")]
pub enum ImagePullPolicy {
    /// Always pull the image
    Always,

    /// Never pull the image
    Never,

    /// Pull the image if not present locally
    #[default]
    IfNotPresent,
}
