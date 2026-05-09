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
}

impl ImageSet {
    pub fn from_config(config: &E2eConfig) -> Self {
        Self {
            operator: config.operator_image.clone(),
            console_web: config.console_web_image.clone(),
            rustfs: config.rustfs_image.clone(),
        }
    }

    pub fn all(&self) -> [&str; 3] {
        [&self.operator, &self.console_web, &self.rustfs]
    }
}

#[cfg(test)]
mod tests {
    use super::ImageSet;
    use crate::framework::config::E2eConfig;

    #[test]
    fn image_set_tracks_operator_console_and_server_images() {
        let images = ImageSet::from_config(&E2eConfig::from_env());

        assert_eq!(
            images.all(),
            [
                "rustfs/operator:e2e",
                "rustfs/console-web:e2e",
                "rustfs/rustfs:e2e"
            ]
        );
    }
}
