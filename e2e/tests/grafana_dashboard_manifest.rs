// Copyright 2026 RustFS Team
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

use std::{
    collections::{BTreeSet, HashSet},
    path::PathBuf,
    process::{Command, Output},
};

use serde_json::Value as JsonValue;
use serde_yaml_ng::Value as YamlValue;

const DASHBOARD_FILES: [&str; 7] = [
    "grafana-get-data-integrity.json",
    "grafana-get-performance-attribution.json",
    "grafana-get-resource-impact.json",
    "grafana-get-rollout-health.json",
    "grafana-object-data-cache.json",
    "grafana-put-performance-attribution.json",
    "rustfs.json",
];

#[test]
fn bundled_dashboards_are_valid_and_have_unique_uids() {
    let dashboard_dir = repository_root().join("deploy/rustfs-operator/dashboards");
    let actual_files = std::fs::read_dir(&dashboard_dir)
        .expect("dashboard directory is readable")
        .filter_map(|entry| {
            let entry = entry.expect("dashboard directory entry is readable");
            (entry.path().extension().and_then(|value| value.to_str()) == Some("json"))
                .then(|| entry.file_name().to_string_lossy().into_owned())
        })
        .collect::<BTreeSet<_>>();
    let expected_files = DASHBOARD_FILES
        .iter()
        .map(|name| (*name).to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(actual_files, expected_files);

    let mut uids = HashSet::new();
    for filename in DASHBOARD_FILES {
        let contents = std::fs::read_to_string(dashboard_dir.join(filename))
            .unwrap_or_else(|error| panic!("failed to read {filename}: {error}"));
        let dashboard: JsonValue = serde_json::from_str(&contents)
            .unwrap_or_else(|error| panic!("{filename} is not valid JSON: {error}"));
        let title = dashboard["title"]
            .as_str()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| panic!("{filename} is missing a title"));
        let uid = dashboard["uid"]
            .as_str()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| panic!("{filename} is missing a uid"));

        assert!(uids.insert(uid.to_owned()), "duplicate dashboard uid {uid}");
        assert!(
            dashboard["templating"]["list"]
                .as_array()
                .is_some_and(|variables| variables.iter().any(|variable| {
                    variable["type"] == "datasource" && variable["query"] == "prometheus"
                })),
            "{title} must select a Prometheus data source"
        );
    }
}

#[test]
fn helm_provisions_dashboards_by_default_and_honors_configuration() {
    let Some(default_render) = helm_template(&["--namespace", "rustfs-system"]) else {
        return;
    };
    let default_documents = rendered_documents(default_render, "default dashboard render");
    let default_dashboards = dashboard_configmaps(&default_documents);
    assert_eq!(default_dashboards.len(), DASHBOARD_FILES.len());

    let mut rendered_files = BTreeSet::new();
    let mut rendered_names = HashSet::new();
    for dashboard in default_dashboards {
        assert_eq!(dashboard["metadata"]["namespace"], "rustfs-system");
        assert_eq!(dashboard["metadata"]["labels"]["grafana_dashboard"], "1");

        let name = dashboard["metadata"]["name"]
            .as_str()
            .expect("dashboard ConfigMap has a name");
        assert!(name.len() <= 63, "ConfigMap name exceeds 63 characters");
        assert!(
            rendered_names.insert(name.to_owned()),
            "duplicate ConfigMap name {name}"
        );

        let data = dashboard["data"]
            .as_mapping()
            .expect("dashboard ConfigMap has data");
        assert_eq!(data.len(), 1);
        let (filename, contents) = data.iter().next().expect("dashboard data is not empty");
        let filename = filename
            .as_str()
            .expect("dashboard data key is a string")
            .to_owned();
        let contents = contents.as_str().expect("dashboard data value is a string");
        serde_json::from_str::<JsonValue>(contents)
            .unwrap_or_else(|error| panic!("rendered {filename} is invalid JSON: {error}"));
        rendered_files.insert(filename);
    }
    assert_eq!(
        rendered_files,
        DASHBOARD_FILES
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<BTreeSet<_>>()
    );

    let disabled_render = helm_template(&["--set", "dashboard.enabled=false"])
        .expect("helm was available for the default render");
    let disabled_documents = rendered_documents(disabled_render, "disabled dashboard render");
    assert!(dashboard_configmaps(&disabled_documents).is_empty());

    let custom_render = helm_template(&[
        "--set",
        "dashboard.namespace=monitoring",
        "--set-string",
        "dashboard.additionalLabels.team=storage",
    ])
    .expect("helm was available for the default render");
    let custom_documents = rendered_documents(custom_render, "custom dashboard render");
    let custom_dashboards = dashboard_configmaps(&custom_documents);
    assert_eq!(custom_dashboards.len(), DASHBOARD_FILES.len());
    for dashboard in custom_dashboards {
        assert_eq!(dashboard["metadata"]["namespace"], "monitoring");
        assert_eq!(dashboard["metadata"]["labels"]["team"], "storage");
    }
}

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("e2e crate has a repository parent")
        .to_path_buf()
}

fn helm_template(arguments: &[&str]) -> Option<Output> {
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

fn rendered_documents(output: Output, description: &str) -> Vec<YamlValue> {
    assert!(
        output.status.success(),
        "{description} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("helm output is UTF-8")
        .split("---")
        .filter(|document| !document.trim().is_empty())
        .map(|document| {
            serde_yaml_ng::from_str(document)
                .unwrap_or_else(|error| panic!("{description} contains invalid YAML: {error}"))
        })
        .collect()
}

fn dashboard_configmaps(documents: &[YamlValue]) -> Vec<&YamlValue> {
    documents
        .iter()
        .filter(|document| {
            document["kind"] == "ConfigMap"
                && document["metadata"]["labels"]["app.kubernetes.io/component"]
                    == "grafana-dashboard"
        })
        .collect()
}
