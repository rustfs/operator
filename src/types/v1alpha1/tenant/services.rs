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

use super::Tenant;
use crate::types::v1alpha1::tls::TlsPlan;
use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
use k8s_openapi::apimachinery::pkg::util::intstr;

fn io_service_name(tenant: &Tenant) -> String {
    format!("{}-io", tenant.name())
}

fn console_service_name(tenant: &Tenant) -> String {
    format!("{}-console", tenant.name())
}

impl Tenant {
    /// a new io Service for tenant
    pub fn new_io_service(&self) -> corev1::Service {
        self.new_io_service_with_tls_plan(&TlsPlan::disabled())
    }

    pub fn new_io_service_with_tls_plan(&self, tls_plan: &TlsPlan) -> corev1::Service {
        corev1::Service {
            metadata: metav1::ObjectMeta {
                name: Some(io_service_name(self)),
                namespace: self.namespace().ok(),
                owner_references: Some(vec![self.new_owner_ref()]),
                labels: Some(self.common_labels()),
                ..Default::default()
            },
            spec: Some(corev1::ServiceSpec {
                type_: Some("ClusterIP".to_owned()),
                selector: Some(self.selector_labels()),
                ports: Some(vec![corev1::ServicePort {
                    port: 9000,
                    target_port: Some(intstr::IntOrString::Int(9000)),
                    name: Some(rustfs_service_port_name(tls_plan).to_owned()),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// a new console Service for tenant
    pub fn new_console_service(&self) -> corev1::Service {
        corev1::Service {
            metadata: metav1::ObjectMeta {
                name: Some(console_service_name(self)),
                namespace: self.namespace().ok(),
                owner_references: Some(vec![self.new_owner_ref()]),
                labels: Some(self.common_labels()),
                ..Default::default()
            },
            spec: Some(corev1::ServiceSpec {
                type_: Some("ClusterIP".to_owned()),
                selector: Some(self.selector_labels()),
                ports: Some(vec![corev1::ServicePort {
                    port: 9001,
                    target_port: Some(intstr::IntOrString::Int(9001)),
                    name: Some("http-console".to_owned()),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// a new headless Service for tenant
    pub fn new_headless_service(&self) -> corev1::Service {
        self.new_headless_service_with_tls_plan(&TlsPlan::disabled())
    }

    pub fn new_headless_service_with_tls_plan(&self, tls_plan: &TlsPlan) -> corev1::Service {
        corev1::Service {
            metadata: metav1::ObjectMeta {
                name: Some(self.headless_service_name()),
                namespace: self.namespace().ok(),
                owner_references: Some(vec![self.new_owner_ref()]),
                labels: Some(self.common_labels()),
                ..Default::default()
            },
            spec: Some(corev1::ServiceSpec {
                type_: Some("ClusterIP".to_owned()),
                cluster_ip: Some("None".to_owned()),
                publish_not_ready_addresses: Some(true),
                selector: Some(self.selector_labels()),
                ports: Some(vec![corev1::ServicePort {
                    port: 9000,
                    name: Some(rustfs_service_port_name(tls_plan).to_owned()),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}

fn rustfs_service_port_name(tls_plan: &TlsPlan) -> &'static str {
    if tls_plan.enabled {
        "https-rustfs"
    } else {
        "http-rustfs"
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::types::v1alpha1::tls::TlsPlan;

    fn first_port_name(service: &k8s_openapi::api::core::v1::Service) -> Option<&str> {
        service
            .spec
            .as_ref()?
            .ports
            .as_ref()?
            .first()?
            .name
            .as_deref()
    }

    #[test]
    fn disabled_tls_keeps_rustfs_services_on_http_port_name() {
        let tenant = crate::tests::create_test_tenant(None, None);

        assert_eq!(
            first_port_name(&tenant.new_io_service()),
            Some("http-rustfs")
        );
        assert_eq!(
            first_port_name(&tenant.new_headless_service()),
            Some("http-rustfs")
        );
    }

    #[test]
    fn enabled_tls_switches_rustfs_services_to_https_port_name() {
        let tenant = crate::tests::create_test_tenant(None, None);
        let tls_plan = TlsPlan::for_test("server-tls", "sha256:test");

        assert_eq!(
            first_port_name(&tenant.new_io_service_with_tls_plan(&tls_plan)),
            Some("https-rustfs")
        );
        assert_eq!(
            first_port_name(&tenant.new_headless_service_with_tls_plan(&tls_plan)),
            Some("https-rustfs")
        );
    }
}
