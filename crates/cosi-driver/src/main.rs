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

use std::path::PathBuf;
use std::time::Duration;

use cosi_driver::backend::BackendFactory;
use cosi_driver::driver::{DRIVER_NAME, IdentityService, ProvisionerService};
use cosi_driver::proto::cosi::v1alpha1::{
    identity_server::IdentityServer, provisioner_server::ProvisionerServer,
};
use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

const DEFAULT_ENDPOINT: &str = "unix:///var/lib/cosi/cosi.sock";
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let endpoint = std::env::var("COSI_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());
    let socket_path = parse_unix_endpoint(&endpoint)?;

    if let Some(parent) = socket_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let _ = tokio::fs::remove_file(&socket_path).await;

    let backend = BackendFactory::try_default().await?;
    let identity = IdentityService {
        name: DRIVER_NAME.to_string(),
    };
    let provisioner = ProvisionerService { backend };

    let uds = UnixListener::bind(&socket_path)?;
    let uds_stream = UnixListenerStream::new(uds);

    info!(
        driver = DRIVER_NAME,
        endpoint = %endpoint,
        "starting RustFS COSI driver"
    );

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(async move {
        wait_for_shutdown().await;
        let _ = shutdown_tx.send(());
    });

    Server::builder()
        .add_service(IdentityServer::new(identity))
        .add_service(ProvisionerServer::new(provisioner))
        .serve_with_incoming_shutdown(uds_stream, async {
            let _ = shutdown_rx.await;
            info!("shutdown signal received");
        })
        .await?;

    let _ = tokio::fs::remove_file(&socket_path).await;
    // Allow in-flight RPCs a brief window before process exit.
    tokio::time::sleep(SHUTDOWN_GRACE).await;
    info!("RustFS COSI driver stopped");
    Ok(())
}

fn parse_unix_endpoint(endpoint: &str) -> Result<PathBuf, String> {
    let endpoint = endpoint.trim();
    if let Some(path) = endpoint.strip_prefix("unix://") {
        if path.is_empty() {
            return Err("COSI_ENDPOINT unix path is empty".into());
        }
        return Ok(PathBuf::from(path));
    }
    if endpoint.starts_with('/') {
        return Ok(PathBuf::from(endpoint));
    }
    Err(format!(
        "unsupported COSI_ENDPOINT `{endpoint}` (expected unix:///path/to.sock)"
    ))
}

async fn wait_for_shutdown() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            warn!(error = %err, "failed to install Ctrl+C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(err) => warn!(error = %err, "failed to install SIGTERM handler"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
