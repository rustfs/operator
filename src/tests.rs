//  Copyright 2025 RustFS Team
//
//  Licensed under the Apache License, Version 2.0 (the "License");
//  you may not use this file except in compliance with the License.
//  You may obtain a copy of the License at
//
//      http:www.apache.org/licenses/LICENSE-2.0
//
//  Unless required by applicable law or agreed to in writing, software
//  distributed under the License is distributed on an "AS IS" BASIS,
//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//  See the License for the specific language governing permissions and
//  limitations under the License.

use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;

use crate::types::v1alpha1::persistence::PersistenceConfig;
use crate::types::v1alpha1::pool::Pool;
use crate::types::v1alpha1::tenant::{Tenant, TenantSpec};

// Helper function to create a test tenant (available to submodule tests via super::tests)
pub fn create_test_tenant(
    service_account_name: Option<String>,
    create_service_account_rbac: Option<bool>,
) -> Tenant {
    Tenant {
        metadata: metav1::ObjectMeta {
            name: Some("test-tenant".to_string()),
            namespace: Some("default".to_string()),
            uid: Some("test-uid-123".to_string()),
            ..Default::default()
        },
        spec: TenantSpec {
            pools: vec![Pool {
                name: "pool-0".to_string(),
                servers: 4,
                persistence: PersistenceConfig {
                    volumes_per_server: 4,
                    ..Default::default()
                },
                scheduling: Default::default(),
            }],
            service_account_name,
            create_service_account_rbac,
            ..Default::default()
        },
        status: None,
    }
}
