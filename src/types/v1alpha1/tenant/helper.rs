//  Copyright 2025 RustFS Team
//
//  Licensed under the Apache License, Version 2.0 (the "License");
//  you may not use this file except in compliance with the License.
//  You may obtain a copy of the License at
//
//      http:www.apache.org/licenses/LICENSE-2.0
//
//  Unless required by applicable law or agreed to in writing, software
//  distributed under the License is distributed on an "AS IS" BASIS,
//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//  See the License for the specific language governing permissions and
//  limitations under the License.

/// Default RustFS container image when neither spec.image nor TENANT_RUSTFS_IMAGE is set.
pub const DEFAULT_RUSTFS_IMAGE: &str = "rustfs/rustfs:1.0.0";

pub(crate) fn get_rustfs_image() -> Option<String> {
    std::env::var("TENANT_RUSTFS_IMAGE").ok()
}

/// Returns the RustFS image to use: spec image > TENANT_RUSTFS_IMAGE env > default.
/// Never returns empty; StatefulSet container.image is required by Kubernetes.
pub(crate) fn get_rustfs_image_or_default(explicit: Option<&String>) -> String {
    explicit
        .cloned()
        .or_else(get_rustfs_image)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_RUSTFS_IMAGE.to_string())
}

pub(crate) fn get_rustfs_mount_path() -> Option<String> {
    Some("/data".to_owned())
}

/// Environment variable that overrides the mTLS readiness probe mode (`tcp` / `tcpSocket` vs default `exec` with `curl`).
/// Can be set on the operator process or in `tenant.spec.env` for distroless images without `curl`.
/// Caveat: `tcpSocket` succeeds as soon as port 9000 accepts TCP connections, before `/health/ready`
/// confirms storage read quorum (tracked in https://github.com/rustfs/rustfs/issues/9256).
pub const TENANT_MTLS_READINESS_PROBE_ENV: &str = "TENANT_MTLS_READINESS_PROBE";
pub const RUSTFS_MTLS_READINESS_PROBE_ENV: &str = "RUSTFS_MTLS_READINESS_PROBE";

pub(crate) fn is_tcp_mtls_readiness_probe_value(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.eq_ignore_ascii_case("tcp") || trimmed.eq_ignore_ascii_case("tcpsocket")
}

pub(crate) fn use_tcp_mtls_readiness_probe(
    explicit_env: &[k8s_openapi::api::core::v1::EnvVar],
) -> bool {
    if let Some(value) = explicit_env
        .iter()
        .rev()
        .find(|env| {
            env.name == TENANT_MTLS_READINESS_PROBE_ENV
                || env.name == RUSTFS_MTLS_READINESS_PROBE_ENV
        })
        .and_then(|env| env.value.as_deref())
    {
        return is_tcp_mtls_readiness_probe_value(value);
    }

    std::env::var(TENANT_MTLS_READINESS_PROBE_ENV)
        .ok()
        .or_else(|| std::env::var(RUSTFS_MTLS_READINESS_PROBE_ENV).ok())
        .as_deref()
        .is_some_and(is_tcp_mtls_readiness_probe_value)
}
