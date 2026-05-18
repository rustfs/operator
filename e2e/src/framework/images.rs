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

use crate::framework::config::E2eConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageSet {
    pub operator: String,
    pub console_web: String,
    pub rustfs: String,
    pub cert_manager_controller: String,
    pub cert_manager_webhook: String,
    pub cert_manager_cainjector: String,
    pub cert_manager_acmesolver: String,
}

impl ImageSet {
    pub fn from_config(config: &E2eConfig) -> Self {
        let cert_manager_version = &config.cert_manager_version;
        Self {
            operator: config.operator_image.clone(),
            console_web: config.console_web_image.clone(),
            rustfs: config.rustfs_image.clone(),
            cert_manager_controller: format!(
                "quay.io/jetstack/cert-manager-controller:{cert_manager_version}"
            ),
            cert_manager_webhook: format!(
                "quay.io/jetstack/cert-manager-webhook:{cert_manager_version}"
            ),
            cert_manager_cainjector: format!(
                "quay.io/jetstack/cert-manager-cainjector:{cert_manager_version}"
            ),
            cert_manager_acmesolver: format!(
                "quay.io/jetstack/cert-manager-acmesolver:{cert_manager_version}"
            ),
        }
    }

    pub fn all(&self) -> [&str; 7] {
        [
            &self.operator,
            &self.console_web,
            &self.rustfs,
            &self.cert_manager_controller,
            &self.cert_manager_webhook,
            &self.cert_manager_cainjector,
            &self.cert_manager_acmesolver,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::ImageSet;
    use crate::framework::config::E2eConfig;

    #[test]
    fn image_set_tracks_operator_console_and_server_images() {
        let images = ImageSet::from_config(&E2eConfig::defaults());

        assert_eq!(
            images.all(),
            [
                "rustfs/operator:e2e",
                "rustfs/console-web:e2e",
                "rustfs/rustfs:latest",
                "quay.io/jetstack/cert-manager-controller:v1.16.2",
                "quay.io/jetstack/cert-manager-webhook:v1.16.2",
                "quay.io/jetstack/cert-manager-cainjector:v1.16.2",
                "quay.io/jetstack/cert-manager-acmesolver:v1.16.2"
            ]
        );
    }
}
