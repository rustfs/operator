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
use rustfs_operator_e2e::framework::{
    command::CommandSpec, config::E2eConfig, deploy, images::ImageSet, kind::KindCluster, live,
};

fn main() -> Result<()> {
    let command = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "help".to_string());
    let config = E2eConfig::from_env();

    match command.as_str() {
        "help" | "--help" | "-h" => print_help(),
        "assert-context" => assert_context(&config),
        "kind-create" => create_kind_cluster(&config),
        "kind-delete" => delete_kind_cluster(&config),
        "kind-load-images" => load_images(&config),
        "deploy-dev" => deploy_dev(&config),
        "rollout-dev" => rollout_dev(&config),
        unknown => bail!("unknown rustfs-e2e internal command: {unknown}; run `rustfs-e2e help`"),
    }
}

fn print_help() -> Result<()> {
    println!("RustFS Operator e2e internal helper");
    println!();
    println!("Operator-facing workflow:");
    println!("  make e2e-live-create");
    println!("  make e2e-live-run");
    println!("  make e2e-live-update");
    println!("  make e2e-live-delete");
    println!();
    println!("Makefile-internal commands:");
    println!("  assert-context    Require RUSTFS_E2E_LIVE=1 and dedicated Kind context");
    println!("  kind-create       Create the dedicated Kind cluster");
    println!("  kind-delete       Delete the dedicated Kind cluster and storage");
    println!("  kind-load-images  Load operator, console-web, and RustFS images into Kind");
    println!("  deploy-dev        Apply operator/console manifests into dedicated Kind");
    println!("  rollout-dev       Restart and wait for e2e control-plane deployments");
    Ok(())
}

fn assert_context(config: &E2eConfig) -> Result<()> {
    live::require_live_enabled(config)?;
    let context = live::ensure_dedicated_context(config)?;
    println!("confirmed dedicated e2e context: {context}");
    Ok(())
}

fn create_kind_cluster(config: &E2eConfig) -> Result<()> {
    live::require_live_enabled(config)?;
    let kind = KindCluster::new(config.clone());
    kind.reset_host_storage_dirs()?;
    kind.create_command().run_checked()?;
    Ok(())
}

fn delete_kind_cluster(config: &E2eConfig) -> Result<()> {
    live::require_live_enabled(config)?;
    let kind = KindCluster::new(config.clone());
    kind.delete_command().run_checked()?;
    kind.cleanup_host_storage_dirs()?;
    Ok(())
}

fn load_images(config: &E2eConfig) -> Result<()> {
    live::require_live_enabled(config)?;

    let images = ImageSet::from_config(config);
    for image in images.all() {
        if !host_image_exists(image) {
            println!("pulling {image} to host");
            CommandSpec::new("docker")
                .args(["pull", image])
                .run_checked()?;
        }
    }

    let kind = KindCluster::new(config.clone());
    for image in images.all() {
        println!("loading {image} into {}", config.cluster_name);
        kind.load_image_command(image).run_checked()?;
    }
    Ok(())
}

fn host_image_exists(image: &str) -> bool {
    match CommandSpec::new("docker")
        .args(["image", "inspect", image])
        .run()
    {
        Ok(output) => output.code == Some(0),
        Err(_) => false,
    }
}

fn deploy_dev(config: &E2eConfig) -> Result<()> {
    live::require_live_enabled(config)?;
    live::ensure_dedicated_context(config)?;
    deploy::deploy_dev(config)
}

fn rollout_dev(config: &E2eConfig) -> Result<()> {
    live::require_live_enabled(config)?;
    live::ensure_dedicated_context(config)?;
    deploy::rollout_dev(config)
}
