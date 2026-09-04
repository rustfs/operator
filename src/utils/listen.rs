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

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use tokio::net::TcpListener;

pub(crate) const OPERATOR_BIND_ADDRESS_ENV: &str = "OPERATOR_BIND_ADDRESS";
pub(crate) const CONSOLE_BIND_ADDRESS_ENV: &str = "CONSOLE_BIND_ADDRESS";

/// Bind an HTTP listener for operator-owned process sockets.
///
/// An explicit IPv4 or IPv6 address in `bind_address_env` always wins. Otherwise the listener
/// prefers the IPv6 unspecified address (`::`), which is dual-stack on typical Linux kernels,
/// and falls back to IPv4 (`0.0.0.0`) on IPv4-only nodes.
pub(crate) async fn bind_unspecified_listener(
    port: u16,
    bind_address_env: &str,
) -> io::Result<TcpListener> {
    let mut last_error = None;
    for addr in listen_addrs(port, bind_address_env)? {
        match TcpListener::bind(addr).await {
            Ok(listener) => return Ok(listener),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            format!("no listen address available for port {port}"),
        )
    }))
}

fn listen_addrs(port: u16, bind_address_env: &str) -> io::Result<Vec<SocketAddr>> {
    if let Some(ip) = explicit_bind_ip(bind_address_env)? {
        return Ok(vec![SocketAddr::from((ip, port))]);
    }
    Ok(vec![
        SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)),
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, port)),
    ])
}

fn explicit_bind_ip(bind_address_env: &str) -> io::Result<Option<IpAddr>> {
    match std::env::var(bind_address_env) {
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            trimmed.parse::<IpAddr>().map(Some).map_err(|error| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid {bind_address_env} value '{trimmed}': {error}"),
                )
            })
        }
        Err(_) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CONSOLE_BIND_ADDRESS_ENV, OPERATOR_BIND_ADDRESS_ENV, bind_unspecified_listener,
        listen_addrs,
    };
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
    use std::sync::{Mutex, MutexGuard, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn env_lock() -> MutexGuard<'static, ()> {
        ENV_LOCK
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn restore_env(name: &str, previous: Option<String>) {
        match previous {
            Some(value) => unsafe { std::env::set_var(name, value) },
            None => unsafe { std::env::remove_var(name) },
        }
    }

    #[test]
    fn auto_listen_addrs_prefer_ipv6_unspecified_then_ipv4() {
        let _guard = env_lock();
        let previous = std::env::var(OPERATOR_BIND_ADDRESS_ENV).ok();
        unsafe { std::env::remove_var(OPERATOR_BIND_ADDRESS_ENV) };

        let addrs = listen_addrs(8080, OPERATOR_BIND_ADDRESS_ENV).expect("auto addrs");
        restore_env(OPERATOR_BIND_ADDRESS_ENV, previous);

        assert_eq!(
            addrs,
            vec![
                SocketAddr::from((Ipv6Addr::UNSPECIFIED, 8080)),
                SocketAddr::from((Ipv4Addr::UNSPECIFIED, 8080)),
            ]
        );
    }

    #[test]
    fn explicit_ipv4_bind_address_is_the_only_candidate() {
        let _guard = env_lock();
        let previous = std::env::var(OPERATOR_BIND_ADDRESS_ENV).ok();
        unsafe { std::env::set_var(OPERATOR_BIND_ADDRESS_ENV, "127.0.0.1") };

        let addrs = listen_addrs(9090, OPERATOR_BIND_ADDRESS_ENV).expect("explicit addrs");
        restore_env(OPERATOR_BIND_ADDRESS_ENV, previous);

        assert_eq!(
            addrs,
            vec![SocketAddr::from((IpAddr::V4(Ipv4Addr::LOCALHOST), 9090))]
        );
    }

    #[test]
    fn explicit_ipv6_bind_address_is_the_only_candidate() {
        let _guard = env_lock();
        let previous = std::env::var(CONSOLE_BIND_ADDRESS_ENV).ok();
        unsafe { std::env::set_var(CONSOLE_BIND_ADDRESS_ENV, "::1") };

        let addrs = listen_addrs(4223, CONSOLE_BIND_ADDRESS_ENV).expect("explicit addrs");
        restore_env(CONSOLE_BIND_ADDRESS_ENV, previous);

        assert_eq!(
            addrs,
            vec![SocketAddr::from((IpAddr::V6(Ipv6Addr::LOCALHOST), 4223))]
        );
    }

    #[test]
    fn invalid_bind_address_is_rejected_before_listen() {
        let _guard = env_lock();
        let previous = std::env::var(OPERATOR_BIND_ADDRESS_ENV).ok();
        unsafe { std::env::set_var(OPERATOR_BIND_ADDRESS_ENV, "not-an-ip") };

        let error = listen_addrs(80, OPERATOR_BIND_ADDRESS_ENV).expect_err("invalid IP");
        restore_env(OPERATOR_BIND_ADDRESS_ENV, previous);

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("OPERATOR_BIND_ADDRESS"));
    }

    #[tokio::test]
    async fn bind_unspecified_listener_accepts_ipv4_loopback() {
        let _guard = env_lock();
        let previous = std::env::var(OPERATOR_BIND_ADDRESS_ENV).ok();
        unsafe { std::env::set_var(OPERATOR_BIND_ADDRESS_ENV, "127.0.0.1") };

        let listener = bind_unspecified_listener(0, OPERATOR_BIND_ADDRESS_ENV)
            .await
            .expect("loopback listener");
        restore_env(OPERATOR_BIND_ADDRESS_ENV, previous);

        let addr = listener.local_addr().expect("bound address");
        assert_eq!(addr.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_ne!(addr.port(), 0);
    }
}
