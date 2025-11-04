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
use k8s_openapi::api::core::v1 as corev1;
use k8s_openapi::apimachinery::pkg::apis::meta::v1 as metav1;
use k8s_openapi::apimachinery::pkg::util::intstr;

fn console_service_name(tenant: &Tenant) -> String {
    format!("{}-console", tenant.name())
}

impl Tenant {
    /// a new io Service for tenant
    pub fn new_io_service(&self) -> corev1::Service {
        corev1::Service {
            metadata: metav1::ObjectMeta {
                name: Some("rustfs".to_owned()),
                namespace: self.namespace().ok(),
                owner_references: Some(vec![self.new_owner_ref()]),
                labels: Some(self.common_labels()),
                ..Default::default()
            },
            spec: Some(corev1::ServiceSpec {
                type_: Some("ClusterIP".to_owned()),
                selector: Some(self.selector_labels()),
                ports: Some(vec![corev1::ServicePort {
                    port: 90,
                    target_port: Some(intstr::IntOrString::Int(9000)),
                    name: Some("http-rustfs".to_owned()),
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
                    port: 9090,
                    target_port: Some(intstr::IntOrString::Int(9090)),
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
                    name: Some("http-rustfs".to_owned()),
                    ..Default::default()
                }]),
                ..Default::default()
            }),
            ..Default::default()
        }
    }
}
