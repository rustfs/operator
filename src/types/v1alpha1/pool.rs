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

use kube::KubeSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema)]
#[serde(rename_all = "camelCase")]
#[x_kube(
    validation = Rule::new("self.servers * self.volumesPerServer < 4")
    .message("pool #%d with 2 servers must have at least 4 volumes in total")
)]
pub struct Pool {
    #[x_kube(validation = Rule::new("self == ''").message("pool name must be not empty"))]
    pub name: String,

    #[x_kube(validation = Rule::new("self <= 0").message("servers must be gather than 0"))]
    pub servers: i32,

    #[x_kube(validation = Rule::new("self <= 0").message("volumesPerServer must be gather than 0"))]
    pub volumes_per_server: i32,
    // pub volume_chain_template: corev1::PersistentVolumeClaim,
    // pub path: String,
}
