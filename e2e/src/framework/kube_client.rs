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

use anyhow::Result;
use kube::{Api, Client};
use operator::types::v1alpha1::tenant::Tenant;

pub async fn default_client() -> Result<Client> {
    Ok(Client::try_default().await?)
}

pub fn tenant_api(client: Client, namespace: &str) -> Api<Tenant> {
    Api::namespaced(client, namespace)
}
