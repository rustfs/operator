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

//! Tests for the Tenant/kube-specific wrappers around `RustfsAdminClient`.
//! Wire-protocol tests (signing, hashing, response parsing) live in the
//! `rustfs-admin` crate.

use super::tls_tenant_base_url;

#[test]
fn tls_tenant_base_url_uses_custom_cluster_domain() {
    let mut tenant = crate::tests::create_test_tenant(None, None);
    tenant.metadata.name = Some("prod-rustfs".to_string());
    tenant.metadata.namespace = Some("mse".to_string());

    assert_eq!(
        tls_tenant_base_url(&tenant, "k8s.mse.cloud").unwrap(),
        "https://prod-rustfs-hl.mse.svc.k8s.mse.cloud:9000"
    );
}
