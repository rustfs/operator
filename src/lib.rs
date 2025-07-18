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

use crate::context::Context;
use crate::error_policy::error_policy;
use crate::reconcile::reconcile;
use crate::types::v1alpha1::tenant::Tenant;
use futures::StreamExt;
use kube::CustomResourceExt;
use kube::runtime::{Controller, watcher};
use kube::{Api, Client};
use std::pin::Pin;
use std::sync::Arc;
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tracing::{info, warn};

mod context;
pub mod error;
pub mod error_policy;
pub mod reconcile;
pub mod types;

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_level(true).init();

    let client = Client::try_default().await?;
    let tenant_client = Api::<Tenant>::all(client.clone());

    let context = Context::new(client);
    Controller::new(tenant_client, watcher::Config::default())
        .run(reconcile, error_policy, Arc::new(context))
        .for_each(|res| async move {
            match res {
                Ok((tenant, _)) => info!("reconciled successful, object{:?}", tenant.name),
                Err(e) => warn!("reconcile failed: {}", e),
            }
        })
        .await;

    Ok(())
}

pub async fn crd(file: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let mut writer: Pin<Box<dyn AsyncWrite + Send>> = if let Some(file) = file {
        Box::pin(
            tokio::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(file)
                .await?,
        )
    } else {
        Box::pin(tokio::io::stdout())
    };

    writer
        .write_all(serde_yaml::to_string(&Tenant::crd())?.as_bytes())
        .await?;

    Ok(())
}
