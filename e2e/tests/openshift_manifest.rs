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

use serde_yaml_ng::Value;
use std::{path::PathBuf, process::Command};

#[test]
fn openshift_values_schema_declares_boolean_switch() {
    let schema = std::fs::read_to_string(
        repository_root().join("deploy/rustfs-operator/values.schema.json"),
    )
    .expect("Helm values schema exists");
    let schema: serde_json::Value =
        serde_json::from_str(&schema).expect("Helm values schema is valid JSON");

    assert_eq!(
        schema["properties"]["openshift"]["properties"]["enabled"]["type"].as_str(),
        Some("boolean")
    );
}

#[test]
fn openshift_chart_mode_delegates_deployment_security_contexts_to_scc() {
    let Some(default_render) = helm_template(&["--set", "console.frontend.enabled=true"]) else {
        return;
    };
    assert!(
        default_render.status.success(),
        "default chart render failed: {}",
        String::from_utf8_lossy(&default_render.stderr)
    );
    let default_output =
        String::from_utf8(default_render.stdout).expect("default helm output is UTF-8");
    let default_documents = yaml_documents(&default_output, "default chart");
    assert_eq!(
        deployment(&default_documents, "rustfs-operator")["spec"]["template"]["spec"]
            ["securityContext"]["fsGroup"]
            .as_i64(),
        Some(65534)
    );
    assert_eq!(
        deployment_container(&default_documents, "rustfs-operator")["securityContext"]["runAsUser"]
            .as_i64(),
        Some(65534)
    );
    assert_eq!(
        deployment_container(&default_documents, "rustfs-operator-console")["securityContext"]
            ["runAsUser"]
            .as_i64(),
        Some(65534)
    );
    assert_eq!(
        deployment_container(&default_documents, "rustfs-operator-console-frontend")
            ["securityContext"]["runAsUser"]
            .as_i64(),
        Some(101)
    );
    for name in [
        "rustfs-operator",
        "rustfs-operator-console",
        "rustfs-operator-console-frontend",
    ] {
        assert!(
            deployment(&default_documents, name)["spec"]["template"]["spec"]["hostUsers"].is_null(),
            "default chart must not pin hostUsers on {name}"
        );
    }

    let openshift_render = helm_template(&[
        "--set",
        "openshift.enabled=true",
        "--set",
        "console.frontend.enabled=true",
    ])
    .expect("helm was available for the default render");
    assert!(
        openshift_render.status.success(),
        "OpenShift chart render failed: {}",
        String::from_utf8_lossy(&openshift_render.stderr)
    );
    let openshift_output =
        String::from_utf8(openshift_render.stdout).expect("OpenShift helm output is UTF-8");
    let openshift_documents = yaml_documents(&openshift_output, "OpenShift chart");

    for name in [
        "rustfs-operator",
        "rustfs-operator-console",
        "rustfs-operator-console-frontend",
    ] {
        let deployment = deployment(&openshift_documents, name);
        assert!(
            deployment["spec"]["template"]["spec"]["securityContext"].is_null(),
            "OpenShift mode must omit the {name} Pod securityContext"
        );
        assert_eq!(
            deployment["spec"]["template"]["spec"]["hostUsers"].as_bool(),
            Some(false),
            "OpenShift mode must set hostUsers: false on {name}"
        );
        for container in deployment["spec"]["template"]["spec"]["containers"]
            .as_sequence()
            .expect("Deployment containers are a sequence")
        {
            assert!(
                container["securityContext"].is_null(),
                "OpenShift mode must omit the {name} container securityContext"
            );
        }
    }
}

#[test]
fn openshift_tenant_example_uses_explicit_empty_pool_security_contexts() {
    let manifest =
        std::fs::read_to_string(repository_root().join("examples/openshift-tenant.yaml"))
            .expect("OpenShift Tenant example exists");
    let documents = yaml_documents(&manifest, "OpenShift Tenant example");
    let tenant = documents
        .iter()
        .find(|document| document["kind"].as_str() == Some("Tenant"))
        .expect("example contains a Tenant");
    let pool = &tenant["spec"]["pools"][0];
    assert_eq!(tenant["spec"]["hostUsers"].as_bool(), Some(false));

    for field in ["securityContext", "containerSecurityContext"] {
        let value = pool[field]
            .as_mapping()
            .unwrap_or_else(|| panic!("{field} must be an object"));
        assert!(value.is_empty(), "{field} must be an explicit empty object");
    }
}

#[test]
fn chart_network_values_are_copied_to_operator_services() {
    let Some(render) = helm_template(&[
        "--set",
        "network.ipFamilyPolicy=PreferDualStack",
        "--set",
        "network.ipFamilies={IPv4,IPv6}",
    ]) else {
        return;
    };
    assert!(
        render.status.success(),
        "network chart render failed: {}",
        String::from_utf8_lossy(&render.stderr)
    );
    let output = String::from_utf8(render.stdout).expect("helm output is UTF-8");
    let documents = yaml_documents(&output, "network chart");
    for name in [
        "rustfs-operator-metrics",
        "rustfs-operator-sts",
        "rustfs-operator-console",
    ] {
        let service = documents
            .iter()
            .find(|document| {
                document["kind"].as_str() == Some("Service")
                    && document["metadata"]["name"].as_str() == Some(name)
            })
            .unwrap_or_else(|| panic!("missing Service {name}"));
        assert_eq!(
            service["spec"]["ipFamilyPolicy"].as_str(),
            Some("PreferDualStack"),
            "{name} should copy ipFamilyPolicy"
        );
        let families = service["spec"]["ipFamilies"]
            .as_sequence()
            .unwrap_or_else(|| panic!("{name} ipFamilies"));
        assert_eq!(
            families
                .iter()
                .map(|item| item.as_str().unwrap_or_default())
                .collect::<Vec<_>>(),
            vec!["IPv4", "IPv6"]
        );
    }
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("e2e crate has a repository parent")
        .to_path_buf()
}

fn helm_template(arguments: &[&str]) -> Option<std::process::Output> {
    if Command::new("helm").arg("version").output().is_err() {
        assert!(
            std::env::var_os("CI").is_none(),
            "helm must be installed in CI"
        );
        eprintln!("skipping helm template assertions: helm is not installed");
        return None;
    }

    Some(
        Command::new("helm")
            .arg("template")
            .arg("rustfs-operator")
            .arg(repository_root().join("deploy/rustfs-operator"))
            .args(arguments)
            .output()
            .expect("helm template runs"),
    )
}

fn yaml_documents(input: &str, description: &str) -> Vec<Value> {
    input
        .split("---")
        .filter(|document| !document.trim().is_empty())
        .map(|document| {
            serde_yaml_ng::from_str(document)
                .unwrap_or_else(|error| panic!("{description} contains invalid YAML: {error}"))
        })
        .collect()
}

fn deployment<'a>(documents: &'a [Value], name: &str) -> &'a Value {
    documents
        .iter()
        .find(|document| {
            document["kind"].as_str() == Some("Deployment")
                && document["metadata"]["name"].as_str() == Some(name)
        })
        .unwrap_or_else(|| panic!("missing Deployment {name}"))
}

fn deployment_container<'a>(documents: &'a [Value], name: &str) -> &'a Value {
    &deployment(documents, name)["spec"]["template"]["spec"]["containers"][0]
}
