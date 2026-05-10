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

use crate::framework::{config::E2eConfig, kubectl::Kubectl, tenant_factory::TenantTemplate};

const E2E_ACCESS_KEY: &str = "e2eaccess";
const E2E_SECRET_KEY: &str = "e2esecret";

pub fn credential_secret_name(config: &E2eConfig) -> String {
    format!("{}-credentials", config.tenant_name)
}

pub fn namespace_manifest(namespace: &str) -> String {
    format!(
        r#"apiVersion: v1
kind: Namespace
metadata:
  name: {namespace}
"#
    )
}

pub fn credential_secret_manifest(config: &E2eConfig) -> String {
    format!(
        r#"apiVersion: v1
kind: Secret
metadata:
  name: {secret_name}
  namespace: {namespace}
type: Opaque
stringData:
  accesskey: {access_key}
  secretkey: {secret_key}
"#,
        secret_name = credential_secret_name(config),
        namespace = config.test_namespace,
        access_key = E2E_ACCESS_KEY,
        secret_key = E2E_SECRET_KEY
    )
}

pub fn smoke_tenant_template(config: &E2eConfig) -> TenantTemplate {
    TenantTemplate::kind_local(
        &config.test_namespace,
        &config.tenant_name,
        &config.rustfs_image,
        &config.storage_class,
        credential_secret_name(config),
    )
}

pub fn smoke_tenant_manifest(config: &E2eConfig) -> Result<String> {
    Ok(serde_yaml_ng::to_string(
        &smoke_tenant_template(config).build(),
    )?)
}

pub fn apply_smoke_tenant_resources(config: &E2eConfig) -> Result<()> {
    let kubectl = Kubectl::new(config);
    kubectl
        .apply_yaml_command(namespace_manifest(&config.test_namespace))
        .run_checked()?;
    kubectl
        .apply_yaml_command(credential_secret_manifest(config))
        .run_checked()?;
    kubectl
        .apply_yaml_command(smoke_tenant_manifest(config)?)
        .run_checked()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{credential_secret_manifest, credential_secret_name, smoke_tenant_manifest};
    use crate::framework::config::E2eConfig;

    #[test]
    fn smoke_tenant_manifest_wires_secret_storage_and_image() {
        let config = E2eConfig::defaults();
        let manifest = smoke_tenant_manifest(&config).expect("tenant manifest");

        assert!(manifest.contains("kind: Tenant"));
        assert!(manifest.contains("namespace: rustfs-e2e-smoke"));
        assert!(manifest.contains("image: rustfs/rustfs:latest"));
        assert!(manifest.contains("storageClassName: local-storage"));
        assert!(manifest.contains("name: e2e-tenant-credentials"));
    }

    #[test]
    fn credential_secret_uses_e2e_tenant_scope() {
        let config = E2eConfig::defaults();
        let manifest = credential_secret_manifest(&config);

        assert_eq!(credential_secret_name(&config), "e2e-tenant-credentials");
        assert!(manifest.contains("namespace: rustfs-e2e-smoke"));
        assert!(manifest.contains("accesskey:"));
        assert!(manifest.contains("secretkey:"));
    }
}
