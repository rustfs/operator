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

use anyhow::{Result, ensure};
use rustfs_operator_e2e::framework::{config::E2eConfig, console_client::ConsoleClient, live};
use serde_json::Value;

#[tokio::test]
#[ignore = "requires Console API reachability; run through `make e2e-console-live` after port-forward/ingress is available"]
async fn console_live_health_ready_and_openapi_are_available() -> Result<()> {
    let config = E2eConfig::from_env();
    live::require_live_enabled(&config)?;
    live::ensure_dedicated_context(&config)?;

    let console = ConsoleClient::new(&config.console_base_url)?;
    let health = console.get_text("/healthz").await?;
    let ready = console.get_text("/readyz").await?;
    let openapi = console.get_json::<Value>("/api-docs/openapi.json").await?;

    ensure!(health.contains("OK"), "unexpected healthz body: {health}");
    ensure!(ready.contains("Ready"), "unexpected readyz body: {ready}");
    ensure!(
        openapi.pointer("/paths/~1api~1v1~1tenants").is_some(),
        "OpenAPI document does not contain /api/v1/tenants"
    );
    ensure!(
        openapi.pointer("/paths/~1api~1v1~1login").is_some(),
        "OpenAPI document does not contain /api/v1/login"
    );

    Ok(())
}
