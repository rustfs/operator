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

use serde::Deserialize;
use serde_yaml_ng::Value;
use std::{
    collections::BTreeMap,
    path::Path,
    process::{Command, Output},
};

fn repository() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("e2e is inside the repository")
}

fn chart_version() -> String {
    let metadata: Value = serde_yaml_ng::from_str(
        &std::fs::read_to_string(repository().join("deploy/rustfs-operator/Chart.yaml"))
            .expect("chart metadata exists"),
    )
    .expect("chart metadata parses");
    metadata["version"]
        .as_str()
        .expect("chart version is a string")
        .to_owned()
}

fn check_metadata(ref_type: &str, ref_name: &str) -> Output {
    Command::new("bash")
        .current_dir(repository())
        .arg("scripts/check-release-metadata.sh")
        .env("GITHUB_REF_TYPE", ref_type)
        .env("GITHUB_REF_NAME", ref_name)
        .output()
        .expect("release metadata check runs")
}

fn successful_output(command: &mut Command) -> Vec<u8> {
    let output = command
        .output()
        .expect("install Helm 3 to run release tests");
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

fn chart_images(chart: &Path, version: &str) -> BTreeMap<String, String> {
    let output = successful_output(
        Command::new("helm")
            .args(["template", "release-test"])
            .arg(chart)
            .args(["--namespace", "rustfs-system"]),
    );
    let mut images = BTreeMap::new();
    for document in serde_yaml_ng::Deserializer::from_slice(&output) {
        let document = Value::deserialize(document).expect("rendered YAML parses");
        if document["kind"] != "Deployment" {
            continue;
        }
        let component = document["metadata"]["labels"]["app.kubernetes.io/component"]
            .as_str()
            .expect("Deployment has a component label");
        if !["operator", "console"].contains(&component) {
            continue;
        }
        let container = document["spec"]["template"]["spec"]["containers"]
            .as_sequence()
            .expect("Deployment has containers")
            .iter()
            .find(|container| container["name"] == component)
            .expect("component container exists");
        let image = container["image"].as_str().expect("container has an image");
        assert!(
            image.ends_with(&format!(":{version}")),
            "{component} image {image} must use release version {version}"
        );
        assert!(
            images
                .insert(component.to_owned(), image.to_owned())
                .is_none()
        );
    }
    assert_eq!(
        images.keys().map(String::as_str).collect::<Vec<_>>(),
        ["console", "operator"]
    );
    images
}

#[test]
fn release_metadata_accepts_branches_and_matching_release_tags() {
    let version = chart_version();
    for (ref_type, ref_name) in [
        ("branch", "main".to_owned()),
        ("tag", version.clone()),
        ("tag", format!("v{version}")),
    ] {
        let output = check_metadata(ref_type, &ref_name);
        assert!(
            output.status.success(),
            "{ref_type} {ref_name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn release_metadata_rejects_mismatched_tags_even_when_files_are_consistent() {
    let version = chart_version();
    let baseline = check_metadata("branch", "main");
    assert!(
        baseline.status.success(),
        "{}",
        String::from_utf8_lossy(&baseline.stderr)
    );
    let mismatched_version = format!("{version}-mismatch");
    for tag in [mismatched_version.clone(), format!("v{mismatched_version}")] {
        let output = check_metadata("tag", &tag);
        assert!(!output.status.success(), "mismatched tag {tag} must fail");
        assert!(String::from_utf8_lossy(&output.stderr).contains(&format!(
            "release tag version is '{mismatched_version}', expected '{version}'"
        )));
    }
}

#[test]
fn release_metadata_requires_a_version_for_tag_refs() {
    for tag in ["", "v"] {
        let output = check_metadata("tag", tag);
        assert!(!output.status.success(), "empty release version must fail");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("could not read release tag version")
        );
    }
}

#[test]
fn source_and_packaged_charts_use_the_release_image_version() {
    let version = if std::env::var("GITHUB_REF_TYPE").as_deref() == Ok("tag") {
        let tag = std::env::var("GITHUB_REF_NAME").expect("tag CI provides the release tag");
        tag.strip_prefix('v').unwrap_or(&tag).to_owned()
    } else {
        chart_version()
    };
    let chart = repository().join("deploy/rustfs-operator");
    let source_images = chart_images(&chart, &version);
    let packages = tempfile::tempdir().expect("chart package directory is created");
    successful_output(
        Command::new("helm")
            .arg("package")
            .arg(&chart)
            .args(["--version", &version, "--app-version", &version])
            .arg("--destination")
            .arg(packages.path()),
    );
    let package = packages
        .path()
        .join(format!("rustfs-operator-{version}.tgz"));
    let metadata: Value = serde_yaml_ng::from_slice(&successful_output(
        Command::new("helm").args(["show", "chart"]).arg(&package),
    ))
    .expect("packaged chart metadata parses");
    assert_eq!(metadata["version"], version);
    assert_eq!(metadata["appVersion"], version);
    assert_eq!(chart_images(&package, &version), source_images);
}
