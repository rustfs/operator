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

use anyhow::{Context, Result, bail, ensure};
use rustfs_operator_e2e::{
    fault::{scenarios::scenario_catalog_json, spec::FaultRunSpec},
    framework::{
        cert_manager_tls, command::CommandSpec, config::E2eConfig, deploy, images::ImageSet,
        kind::KindCluster, live, resources, storage,
    },
};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".to_string());

    match command.as_str() {
        "help" | "--help" | "-h" => print_help(),
        "fault-catalog-json" => print_fault_catalog_json(),
        "fault-run-spec-equal" => validate_fault_run_spec_equivalence(args),
        _ => {
            let config = E2eConfig::from_env();
            match command.as_str() {
                "assert-context" => assert_context(&config),
                "kind-create" => create_kind_cluster(&config),
                "kind-delete" => delete_kind_cluster(&config),
                "sanitize-live-storage" => sanitize_live_storage(&config),
                "reset-live-fixtures" | "reset-live-smoke-fixture" => reset_live_fixtures(&config),
                "kind-load-images" => load_images(&config),
                "deploy-dev" => deploy_dev(&config),
                "rollout-dev" => rollout_dev(&config),
                unknown => {
                    bail!("unknown rustfs-e2e internal command: {unknown}; run `rustfs-e2e help`")
                }
            }
        }
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
    println!("  sanitize-live-storage");
    println!("  reset-live-fixtures");
    println!(
        "  kind-load-images  Load operator, console-web, RustFS, and dependency images into Kind"
    );
    println!("  deploy-dev        Apply operator/console manifests into dedicated Kind");
    println!("  rollout-dev       Restart and wait for e2e control-plane deployments");
    println!("  fault-catalog-json");
    println!("  fault-run-spec-equal <run-spec.json> <run-spec.yaml>");
    Ok(())
}

fn print_fault_catalog_json() -> Result<()> {
    println!("{}", scenario_catalog_json()?);
    Ok(())
}

fn validate_fault_run_spec_equivalence(mut args: impl Iterator<Item = String>) -> Result<()> {
    let json_path = args
        .next()
        .context("fault-run-spec-equal requires run-spec.json path")?;
    let yaml_path = args
        .next()
        .context("fault-run-spec-equal requires run-spec.yaml path")?;
    ensure!(
        args.next().is_none(),
        "fault-run-spec-equal accepts exactly two paths"
    );

    let json_raw = std::fs::read_to_string(&json_path)
        .with_context(|| format!("read run spec json {json_path}"))?;
    let yaml_raw = std::fs::read_to_string(&yaml_path)
        .with_context(|| format!("read run spec yaml {yaml_path}"))?;
    let json_spec = serde_json::from_str::<FaultRunSpec>(&json_raw)
        .with_context(|| format!("parse run spec json {json_path}"))?;
    let yaml_spec = serde_yaml_ng::from_str::<FaultRunSpec>(&yaml_raw)
        .with_context(|| format!("parse run spec yaml {yaml_path}"))?;

    ensure!(
        json_spec == yaml_spec,
        "run spec JSON and YAML artifacts do not describe the same contract"
    );
    println!("run spec JSON/YAML contract matches");
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
    kind.create_command()?.run_checked()?;
    Ok(())
}

fn delete_kind_cluster(config: &E2eConfig) -> Result<()> {
    live::require_live_enabled(config)?;
    let kind = KindCluster::new(config.clone());
    kind.delete_command().run_checked()?;
    kind.cleanup_host_storage_dirs()?;
    Ok(())
}

fn sanitize_live_storage(config: &E2eConfig) -> Result<()> {
    live::require_live_enabled(config)?;
    let kind = KindCluster::new(config.clone());
    let stale_paths = kind.stale_local_rustfs_format_paths()?;

    if stale_paths.is_empty() {
        println!("no stale rustfs format metadata found in dedicated host storage");
        return Ok(());
    }

    println!(
        "detected {} stale rustfs format file(s) in dedicated host storage",
        stale_paths.len()
    );
    for path in stale_paths {
        println!("  - {}", path.display());
    }

    bail!(
        "refusing to reset live host storage while the Kind cluster may still be running; recreate the dedicated e2e cluster with `make e2e-live-create`"
    )
}

fn reset_live_fixtures(config: &E2eConfig) -> Result<()> {
    live::require_live_enabled(config)?;
    live::ensure_dedicated_context(config)?;
    resources::reset_tenant_resources(config)?;
    storage::reset_default_local_storage(config)?;
    cert_manager_tls::reset_positive_case_resources(config)?;
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
        println!("loading {image} into {} nodes", config.cluster_name);
        kind.load_image(image)?;
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
