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
use std::{collections::BTreeSet, path::PathBuf, process::Command};

#[test]
fn operator_rbac_can_update_tenant_finalizers() {
    let root = repository_root();
    let dev_rbac = std::fs::read_to_string(root.join("deploy/k8s-dev/operator-rbac.yaml"))
        .expect("k8s-dev operator RBAC exists");
    let dev_documents = yaml_documents(&dev_rbac, "k8s-dev operator RBAC");
    assert_finalizer_update_rule(&dev_documents, "rustfs-operator");

    let Some(rendered) = helm_template() else {
        return;
    };
    assert!(
        rendered.status.success(),
        "default chart render failed: {}",
        String::from_utf8_lossy(&rendered.stderr)
    );
    let output = String::from_utf8(rendered.stdout).expect("helm output is UTF-8");
    assert_finalizer_update_rule(&yaml_documents(&output, "default chart"), "rustfs-operator");
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("e2e crate has a repository parent")
        .to_path_buf()
}

fn helm_template() -> Option<std::process::Output> {
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

fn assert_finalizer_update_rule(documents: &[Value], name: &str) {
    let cluster_role = documents
        .iter()
        .find(|document| {
            document["kind"].as_str() == Some("ClusterRole")
                && document["metadata"]["name"].as_str() == Some(name)
        })
        .unwrap_or_else(|| panic!("missing ClusterRole {name}"));
    let rules = cluster_role["rules"]
        .as_sequence()
        .expect("ClusterRole rules are a sequence");
    let matching = rules
        .iter()
        .filter(|rule| yaml_strings(&rule["resources"]).contains("tenants/finalizers"))
        .collect::<Vec<_>>();

    assert_eq!(matching.len(), 1, "expected one Tenant finalizer rule");
    assert_eq!(
        yaml_strings(&matching[0]["apiGroups"]),
        BTreeSet::from(["rustfs.com"])
    );
    assert_eq!(
        yaml_strings(&matching[0]["resources"]),
        BTreeSet::from(["tenants/finalizers"])
    );
    assert_eq!(
        yaml_strings(&matching[0]["verbs"]),
        BTreeSet::from(["update"])
    );
}

fn yaml_strings(value: &Value) -> BTreeSet<&str> {
    value
        .as_sequence()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}
