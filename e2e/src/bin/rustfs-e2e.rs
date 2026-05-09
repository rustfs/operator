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
use rustfs_operator_e2e::cases::all_cases;
use rustfs_operator_e2e::framework::{
    command::CommandSpec, config::E2eConfig, deploy, images::ImageSet, kind::KindCluster, live,
    storage, tools,
};

fn main() -> Result<()> {
    let command = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "help".to_string());
    let config = E2eConfig::from_env();

    match command.as_str() {
        "help" | "--help" | "-h" => print_help(),
        "print-config" => print_config(&config),
        "plan" => print_plan(),
        "doctor" => run_doctor(&config),
        "assert-context" => assert_context(&config),
        "kind-create" => create_kind_cluster(&config),
        "kind-delete" => delete_kind_cluster(&config),
        "kind-load-images" => load_images(&config),
        "storage-prepare" => prepare_storage(&config),
        "deploy-dev" => deploy_dev(&config),
        unknown => bail!("unknown rustfs-e2e command: {unknown}; run `rustfs-e2e help`"),
    }
}

fn print_help() -> Result<()> {
    println!("RustFS Operator e2e helper");
    println!();
    println!("Commands:");
    println!("  plan              Print release e2e case inventory");
    println!("  print-config      Print resolved e2e configuration");
    println!("  doctor            Check local host tools and resolved config");
    println!("  assert-context    Require RUSTFS_E2E_LIVE=1 and dedicated kind context");
    println!("  kind-create       Create the configured Kind cluster");
    println!("  kind-delete       Delete the configured Kind cluster");
    println!("  kind-load-images  Load operator, console-web, and RustFS images into Kind");
    println!("  storage-prepare   Create local PV directories and apply e2e StorageClass/PVs");
    println!(
        "  deploy-dev        Apply RustFS operator/console/rbac manifests into dedicated Kind"
    );
    Ok(())
}

fn print_config(config: &E2eConfig) -> Result<()> {
    println!("cluster_name={}", config.cluster_name);
    println!("context={}", config.context);
    println!("operator_namespace={}", config.operator_namespace);
    println!("test_namespace={}", config.test_namespace);
    println!("test_namespace_prefix={}", config.test_namespace_prefix);
    println!("tenant_name={}", config.tenant_name);
    println!("console_base_url={}", config.console_base_url);
    println!("storage_class={}", config.storage_class);
    println!(
        "storage_host_dir_prefix={}",
        config.storage_host_dir_prefix.display()
    );
    println!("pv_count={}", config.pv_count);
    println!("operator_image={}", config.operator_image);
    println!("console_web_image={}", config.console_web_image);
    println!("rustfs_image={}", config.rustfs_image);
    println!("kind_config={}", config.kind_config.display());
    println!("artifacts_dir={}", config.artifacts_dir.display());
    println!("keep_cluster={}", config.keep_cluster);
    println!("skip_build={}", config.skip_build);
    println!("live_enabled={}", config.live_enabled);
    println!("destructive_enabled={}", config.destructive_enabled);
    println!("timeout_seconds={}", config.timeout.as_secs());
    Ok(())
}

fn print_plan() -> Result<()> {
    for case in all_cases() {
        println!(
            "{suite:?}\t{name}\t{boundary}\t{phase}\t{description}",
            suite = case.suite,
            name = case.name,
            boundary = case.boundary,
            phase = case.ci_phase,
            description = case.description
        );
    }
    Ok(())
}

fn run_doctor(config: &E2eConfig) -> Result<()> {
    print_config(config)?;
    println!();
    println!("Host tool checks:");

    let mut failures = Vec::new();
    for (name, result) in tools::run_doctor_checks() {
        match result {
            Ok(summary) => println!("  ✓ {name}: {summary}"),
            Err(error) => {
                println!("  ✗ {name}: {error}");
                failures.push(name);
            }
        }
    }

    if config.live_enabled {
        match live::ensure_dedicated_context(config) {
            Ok(context) => println!("  ✓ context: {context}"),
            Err(error) => {
                println!("  ✗ context: {error}");
                failures.push("context");
            }
        }
    } else {
        println!("  - context: skipped because RUSTFS_E2E_LIVE is not set");
    }

    if failures.is_empty() {
        Ok(())
    } else {
        bail!("e2e doctor failed checks: {failures:?}")
    }
}

fn assert_context(config: &E2eConfig) -> Result<()> {
    live::require_live_enabled(config)?;
    let context = live::ensure_dedicated_context(config)?;
    println!("confirmed dedicated e2e context: {context}");
    Ok(())
}

fn create_kind_cluster(config: &E2eConfig) -> Result<()> {
    let kind = KindCluster::new(config.clone());
    kind.reset_host_storage_dirs()?;
    kind.create_command().run_checked()?;
    Ok(())
}

fn delete_kind_cluster(config: &E2eConfig) -> Result<()> {
    let kind = KindCluster::new(config.clone());
    kind.delete_command().run_checked()?;
    kind.cleanup_host_storage_dirs()?;
    Ok(())
}

fn load_images(config: &E2eConfig) -> Result<()> {
    if !config.skip_build {
        for image in ImageSet::from_config(config).all() {
            if !host_image_exists(image) {
                println!("pulling {image} to host");
                CommandSpec::new("docker")
                    .args(["pull", image])
                    .run_checked()?;
            }
        }
    }

    let kind = KindCluster::new(config.clone());
    let images = ImageSet::from_config(config);
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
fn prepare_storage(config: &E2eConfig) -> Result<()> {
    live::require_live_enabled(config)?;
    live::ensure_dedicated_context(config)?;
    storage::prepare_local_storage(config)
}

fn deploy_dev(config: &E2eConfig) -> Result<()> {
    live::require_live_enabled(config)?;
    live::ensure_dedicated_context(config)?;
    deploy::deploy_dev(config)
}
