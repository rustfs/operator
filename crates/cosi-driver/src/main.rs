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

//! RustFS COSI driver — serves Identity + Provisioner on a Unix socket.

mod backend;
mod bucket;
mod credentials;
mod driver;
mod grant;
mod ownership;
mod parameters;

pub mod proto {
    pub mod cosi {
        pub mod v1alpha1 {
            tonic::include_proto!("cosi.v1alpha1");
        }
    }
}

use std::path::PathBuf;

use kube::Client;
use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tracing::{error, info};

use crate::driver::Driver;
use crate::parameters::DRIVER_NAME;
use crate::proto::cosi::v1alpha1::{
    identity_server::IdentityServer, provisioner_server::ProvisionerServer,
};

fn parse_unix_endpoint(raw: &str) -> Result<PathBuf, String> {
    let trimmed = raw.trim();
    let path = trimmed
        .strip_prefix("unix://")
        .ok_or_else(|| format!("unsupported COSI_ENDPOINT `{trimmed}`"))?;
    if path.is_empty() {
        return Err("COSI_ENDPOINT unix path is empty".to_string());
    }
    Ok(PathBuf::from(path))
}

#[tokio::main]
async fn main() {
    // Required for rustls 0.23 when multiple crypto backends may be linked via deps.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(true)
        .try_init();

    let endpoint = std::env::var("COSI_ENDPOINT")
        .unwrap_or_else(|_| "unix:///var/lib/cosi/cosi.sock".to_string());
    let sock_path = match parse_unix_endpoint(&endpoint) {
        Ok(path) => path,
        Err(err) => {
            error!(error = %err, "invalid COSI_ENDPOINT");
            std::process::exit(2);
        }
    };

    if let Some(parent) = sock_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::remove_file(&sock_path);

    let kube = match Client::try_default().await {
        Ok(client) => client,
        Err(err) => {
            error!(error = %err, "failed to create Kubernetes client");
            std::process::exit(1);
        }
    };

    let listener = match UnixListener::bind(&sock_path) {
        Ok(listener) => listener,
        Err(err) => {
            error!(error = %err, path = %sock_path.display(), "failed to bind COSI socket");
            std::process::exit(1);
        }
    };
    let incoming = UnixListenerStream::new(listener);
    let driver = Driver::new(kube);

    info!(
        driver = DRIVER_NAME,
        endpoint = %endpoint,
        "starting RustFS COSI driver"
    );

    let result = tonic::transport::Server::builder()
        .add_service(IdentityServer::new(driver.clone()))
        .add_service(ProvisionerServer::new(driver))
        .serve_with_incoming(incoming)
        .await;

    if let Err(err) = result {
        error!(error = %err, "RustFS COSI driver stopped");
        std::process::exit(1);
    }
}
