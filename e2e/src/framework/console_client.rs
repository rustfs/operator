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
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

#[derive(Clone)]
pub struct ConsoleClient {
    base_url: String,
    http: Client,
}

impl ConsoleClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let http = Client::builder().cookie_store(true).build()?;
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http,
        })
    }

    pub async fn get_text(&self, path: &str) -> Result<String> {
        Ok(self
            .http
            .get(self.url(path))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?)
    }

    pub async fn get_json<T>(&self, path: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        Ok(self
            .http
            .get(self.url(path))
            .send()
            .await?
            .error_for_status()?
            .json::<T>()
            .await?)
    }

    pub async fn login_with_kubernetes_token(&self, token: &str) -> Result<Value> {
        Ok(self
            .http
            .post(self.url("/api/v1/login"))
            .json(&json!({ "token": token }))
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?)
    }

    pub async fn logout(&self) -> Result<Value> {
        Ok(self
            .http
            .post(self.url("/api/v1/logout"))
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?)
    }

    fn url(&self, path: &str) -> String {
        if path.starts_with('/') {
            format!("{}{}", self.base_url, path)
        } else {
            format!("{}/{}", self.base_url, path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ConsoleClient;

    #[test]
    fn client_normalizes_base_url_without_trailing_slash() {
        let client = ConsoleClient::new("http://127.0.0.1:9090/").expect("client builds");

        assert_eq!(client.url("/healthz"), "http://127.0.0.1:9090/healthz");
        assert_eq!(client.url("readyz"), "http://127.0.0.1:9090/readyz");
    }
}
