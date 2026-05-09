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

use crate::framework::{config::E2eConfig, kubectl::Kubectl, resources};

const CONTROL_PLANE_CRD: &str =
    include_str!("../../../deploy/rustfs-operator/crds/tenant-crd.yaml");
const OPERATOR_RBAC: &str = include_str!("../../../deploy/k8s-dev/operator-rbac.yaml");
const CONSOLE_RBAC: &str = include_str!("../../../deploy/k8s-dev/console-rbac.yaml");
const OPERATOR_DEPLOYMENT: &str = include_str!("../../../deploy/k8s-dev/operator-deployment.yaml");
const CONSOLE_DEPLOYMENT: &str = include_str!("../../../deploy/k8s-dev/console-deployment.yaml");
const CONSOLE_SERVICE: &str = include_str!("../../../deploy/k8s-dev/console-service.yaml");
const CONSOLE_FRONTEND_DEPLOYMENT: &str =
    include_str!("../../../deploy/k8s-dev/console-frontend-deployment.yaml");
const CONSOLE_FRONTEND_SERVICE: &str =
    include_str!("../../../deploy/k8s-dev/console-frontend-service.yaml");

const CONSOLE_JWT_SECRET_NAME: &str = "rustfs-operator-console-secret";
const E2E_OPERATOR_IMAGE_TAG_DEFAULT: &str = "rustfs/operator:dev";
const E2E_CONSOLE_WEB_IMAGE_TAG_DEFAULT: &str = "rustfs/console-web:dev";

pub fn deploy_dev(config: &E2eConfig) -> Result<()> {
    let kubectl = Kubectl::new(config);

    kubectl
        .apply_yaml_command(resources::namespace_manifest(&config.operator_namespace))
        .run_checked()?;

    kubectl
        .apply_yaml_command(CONTROL_PLANE_CRD)
        .run_checked()?;

    kubectl
        .apply_yaml_command(ensure_console_jwt_secret(config))
        .run_checked()?;

    kubectl.apply_yaml_command(OPERATOR_RBAC).run_checked()?;
    kubectl.apply_yaml_command(CONSOLE_RBAC).run_checked()?;

    kubectl
        .apply_yaml_command(patch_images_and_tags(
            OPERATOR_DEPLOYMENT,
            &config.operator_image,
            E2E_OPERATOR_IMAGE_TAG_DEFAULT,
        ))
        .run_checked()?;
    kubectl
        .apply_yaml_command(patch_images_and_tags(
            CONSOLE_DEPLOYMENT,
            &config.operator_image,
            E2E_OPERATOR_IMAGE_TAG_DEFAULT,
        ))
        .run_checked()?;
    kubectl.apply_yaml_command(CONSOLE_SERVICE).run_checked()?;
    kubectl
        .apply_yaml_command(patch_images_and_tags(
            CONSOLE_FRONTEND_DEPLOYMENT,
            &config.console_web_image,
            E2E_CONSOLE_WEB_IMAGE_TAG_DEFAULT,
        ))
        .run_checked()?;
    kubectl
        .apply_yaml_command(CONSOLE_FRONTEND_SERVICE)
        .run_checked()?;

    Ok(())
}

fn ensure_console_jwt_secret(config: &E2eConfig) -> String {
    format!(
        r#"apiVersion: v1
kind: Secret
metadata:
  name: {secret}
  namespace: {namespace}
type: Opaque
stringData:
  jwt-secret: {jwt}
"#,
        secret = CONSOLE_JWT_SECRET_NAME,
        namespace = config.operator_namespace,
        jwt = "rustfs-e2e-jwt-secret",
    )
}

fn patch_images_and_tags(manifest: &str, image: &str, fallback: &str) -> String {
    if image == fallback {
        manifest.to_string()
    } else {
        manifest.replace(fallback, image)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        E2E_CONSOLE_WEB_IMAGE_TAG_DEFAULT, E2E_OPERATOR_IMAGE_TAG_DEFAULT, patch_images_and_tags,
    };

    #[test]
    fn patch_images_prefers_explicit_runtime_image_tags() {
        let operator = patch_images_and_tags(
            "image: rustfs/operator:dev\nimagePullPolicy: Never\n",
            "rustfs/operator:e2e",
            E2E_OPERATOR_IMAGE_TAG_DEFAULT,
        );
        let web = patch_images_and_tags(
            "image: rustfs/console-web:dev\n",
            "rustfs/console-web:e2e",
            E2E_CONSOLE_WEB_IMAGE_TAG_DEFAULT,
        );

        assert!(operator.contains("image: rustfs/operator:e2e"));
        assert!(!operator.contains(E2E_OPERATOR_IMAGE_TAG_DEFAULT));
        assert!(web.contains("image: rustfs/console-web:e2e"));
        assert!(!web.contains(E2E_CONSOLE_WEB_IMAGE_TAG_DEFAULT));
    }
}
