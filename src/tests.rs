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

#![allow(clippy::unwrap_used)]

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

#[cfg(test)]
mod unit_tests {
    use super::*;
    use k8s_openapi::api::core::v1 as corev1;

    #[test]
    fn test_statefulset_generation_with_probes() {
        let mut tenant = create_test_tenant(None, None);
        // Add probes
        tenant.spec.liveness = Some(corev1::Probe {
            initial_delay_seconds: Some(10),
            ..Default::default()
        });

        let ss = tenant.new_statefulset(&tenant.spec.pools[0]).unwrap();
        let container = &ss
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers[0];

        assert!(container.liveness_probe.is_some());
        assert_eq!(
            container
                .liveness_probe
                .as_ref()
                .unwrap()
                .initial_delay_seconds,
            Some(10)
        );
    }

    #[test]
    fn test_pdb_generation() {
        let tenant = create_test_tenant(None, None);
        let pdb = tenant.new_pdb(&tenant.spec.pools[0]).unwrap();

        assert_eq!(pdb.metadata.name.unwrap(), "test-tenant-pool-0");
        assert!(pdb.spec.unwrap().max_unavailable.is_some());
    }

    #[test]
    fn test_statefulset_tls_config() {
        let mut tenant = create_test_tenant(None, None);
        tenant.spec.request_auto_cert = Some(true);

        let ss = tenant.new_statefulset(&tenant.spec.pools[0]).unwrap();
        let container = &ss
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .containers[0];

        // Check Mounts
        let has_tls_mount = container
            .volume_mounts
            .as_ref()
            .unwrap()
            .iter()
            .any(|vm| vm.name == "tls-certs" && vm.mount_path == "/etc/rustfs-tls");
        assert!(has_tls_mount);

        // Check Env
        let envs = container.env.as_ref().unwrap();
        assert!(
            envs.iter()
                .any(|e| e.name == "RUSTFS_TLS_ENABLE" && e.value == Some("true".to_string()))
        );
        assert!(envs.iter().any(|e| e.name == "RUSTFS_TLS_CERT_FILE"));

        // Check Volumes
        let volumes = ss
            .spec
            .as_ref()
            .unwrap()
            .template
            .spec
            .as_ref()
            .unwrap()
            .volumes
            .as_ref()
            .unwrap();
        let has_tls_vol = volumes
            .iter()
            .any(|v| v.name == "tls-certs" && v.secret.is_some());
        assert!(has_tls_vol);
    }
}

#[test]
fn test_volume_resize_detection_logic() {
    use k8s_openapi::api::core::v1 as corev1;
    use k8s_openapi::apimachinery::pkg::api::resource::Quantity;

    let mut tenant = create_test_tenant(None, None);
    // Set initial validation
    let mut template = corev1::PersistentVolumeClaimSpec::default();
    let mut requests = std::collections::BTreeMap::new();
    requests.insert("storage".to_string(), Quantity("10Gi".to_string()));
    template.resources = Some(corev1::VolumeResourceRequirements {
        requests: Some(requests),
        ..Default::default()
    });

    tenant.spec.pools[0].persistence.volume_claim_template = Some(template);

    // Simulation logic similar to resize_pool_pvcs
    let desired = tenant.spec.pools[0]
        .persistence
        .volume_claim_template
        .as_ref()
        .unwrap()
        .resources
        .as_ref()
        .unwrap()
        .requests
        .as_ref()
        .unwrap()
        .get("storage")
        .unwrap();

    assert_eq!(desired.0, "10Gi");

    // Mock current PVC state
    let current_qty = Quantity("5Gi".to_string());

    assert_ne!(&current_qty, desired);
    // Logic would trigger resize here
}
