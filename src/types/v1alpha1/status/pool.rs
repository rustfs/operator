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

use kube::KubeSchema;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use strum::Display;

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema)]
#[serde(rename_all = "camelCase")]
pub struct Pool {
    pub ss_name: String,
    pub state: PoolState,
}

#[derive(Deserialize, Serialize, Clone, Debug, Display)]
pub enum PoolState {
    #[strum(serialize = "PoolCreated")]
    Created,

    #[strum(serialize = "PoolNotCreated")]
    NotCreated,

    #[strum(serialize = "PoolInitialized")]
    Initialized,
}

impl JsonSchema for PoolState {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("State")
    }
    fn schema_id() -> Cow<'static, str> {
        Cow::Borrowed(concat!(module_path!(), "::", "State"))
    }
    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema! {
            {"type": "string"}
        }
    }
}
