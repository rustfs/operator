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

use anyhow::{Result, bail};
use kube::Api;
use operator::types::v1alpha1::tenant::Tenant;
use std::future::Future;
use std::time::{Duration, Instant};
use tokio::time::sleep;

use crate::framework::assertions;

pub async fn wait_until<T, F, Fut>(
    description: &str,
    timeout: Duration,
    interval: Duration,
    mut probe: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Option<T>>>,
{
    let start = Instant::now();
    loop {
        if let Some(value) = probe().await? {
            return Ok(value);
        }

        if start.elapsed() >= timeout {
            bail!("timed out waiting for {description} after {timeout:?}");
        }

        sleep(interval).await;
    }
}

pub async fn wait_for_tenant_ready(
    tenants: Api<Tenant>,
    name: &str,
    timeout: Duration,
) -> Result<Tenant> {
    let name = name.to_string();
    wait_until(
        &format!("Tenant {name} to become Ready"),
        timeout,
        Duration::from_secs(5),
        move || {
            let tenants = tenants.clone();
            let name = name.clone();
            async move {
                let tenant = tenants.get(&name).await?;
                if assertions::current_state(&tenant) == Some("Ready")
                    && assertions::condition_status(&tenant, "Ready") == Some("True")
                    && assertions::condition_status(&tenant, "Degraded") == Some("False")
                    && assertions::require_observed_generation_current(&tenant).is_ok()
                {
                    Ok(Some(tenant))
                } else {
                    Ok(None)
                }
            }
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::wait_until;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[tokio::test]
    async fn wait_until_returns_first_successful_value() {
        let attempts = Arc::new(Mutex::new(0));
        let attempts_for_probe = Arc::clone(&attempts);

        let result = wait_until(
            "counter reaches two",
            Duration::from_secs(1),
            Duration::from_millis(1),
            move || {
                let attempts_for_probe = Arc::clone(&attempts_for_probe);
                async move {
                    let mut guard = attempts_for_probe
                        .lock()
                        .map_err(|_| anyhow::anyhow!("poisoned"))?;
                    *guard += 1;
                    if *guard >= 2 {
                        Ok(Some(*guard))
                    } else {
                        Ok(None)
                    }
                }
            },
        )
        .await
        .expect("wait succeeds");

        assert_eq!(result, 2);
    }
}
