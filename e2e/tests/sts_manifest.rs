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
use std::{
    collections::BTreeSet,
    path::Path,
    process::{Command, Output},
};

const RESERVED_OPERATOR_ENV: &[(&str, &str)] = &[
    ("OPERATOR_CLUSTER_DOMAIN", "clusterDomain"),
    ("OPERATOR_METRICS_ENABLED", "operator.metrics.enabled"),
    ("OPERATOR_METRICS_PORT", "operator.metrics.port"),
    ("OPERATOR_NAMESPACE", "namespace"),
    ("OPERATOR_STS_ENABLED", "sts.enabled"),
    ("OPERATOR_STS_AUDIENCE", "sts.audience"),
    (
        "OPERATOR_STS_ADMISSION_REQUESTS_PER_SECOND",
        "sts.admission.requestsPerSecond",
    ),
    ("OPERATOR_STS_ADMISSION_BURST", "sts.admission.burst"),
    (
        "OPERATOR_STS_ADMISSION_MAX_IN_FLIGHT",
        "sts.admission.maxInFlight",
    ),
    (
        "OPERATOR_STS_ADMISSION_BODY_LIMIT_BYTES",
        "sts.admission.bodyLimitBytes",
    ),
    (
        "OPERATOR_STS_ADMISSION_TIMEOUT_SECONDS",
        "sts.admission.timeoutSeconds",
    ),
    ("OPERATOR_STS_PORT", "sts.port"),
    (
        "OPERATOR_STS_SERVICE_NAME",
        "the generated STS Service name",
    ),
    ("OPERATOR_STS_TLS_ENABLED", "sts.tls.enabled"),
    ("OPERATOR_STS_TLS_AUTO", "sts.tls.auto"),
    (
        "OPERATOR_TENANT_MONITOR_ENABLED",
        "operator.tenantMonitor.enabled",
    ),
    (
        "OPERATOR_TENANT_MONITOR_INTERVAL_SECONDS",
        "operator.tenantMonitor.intervalSeconds",
    ),
    ("POD_NAME", "the Pod metadata.name field"),
];

#[test]
fn k8s_dev_manifests_expose_sts_service_and_rbac_permissions() {
    // CRD/STS-specific RBAC and porting is required for STS flow.
    let k8s_rbac = std::fs::read_to_string("../deploy/k8s-dev/operator-rbac.yaml")
        .expect("k8s dev operator-rbac exists");
    let k8s_deploy = std::fs::read_to_string("../deploy/k8s-dev/operator-deployment.yaml")
        .expect("k8s dev operator deployment exists");
    let k8s_sts_svc = std::fs::read_to_string("../deploy/k8s-dev/operator-sts-service.yaml")
        .expect("k8s dev sts service exists");

    assert!(
        k8s_rbac.contains("policybindings"),
        "k8s-rbac should include policybindings"
    );
    assert!(
        k8s_rbac.contains("tokenreviews"),
        "k8s-rbac should include tokenreviews"
    );
    assert!(k8s_deploy.contains("app.kubernetes.io/component: operator"));
    assert!(k8s_deploy.contains("name: sts"));
    assert!(k8s_deploy.contains("containerPort: 4223"));
    assert!(k8s_deploy.contains("name: OPERATOR_STS_ENABLED"));
    assert!(k8s_deploy.contains("value: \"true\""));
    assert!(k8s_deploy.contains("name: OPERATOR_STS_AUDIENCE"));
    assert!(k8s_deploy.contains("value: sts.rustfs.com"));
    assert!(k8s_deploy.contains("value: \"4223\""));
    assert!(k8s_deploy.contains("name: OPERATOR_NAMESPACE"));
    assert!(k8s_deploy.contains("fieldPath: metadata.namespace"));
    assert!(k8s_deploy.contains("name: OPERATOR_STS_SERVICE_NAME"));
    assert!(k8s_deploy.contains("value: rustfs-operator-sts"));
    assert!(k8s_deploy.contains("name: OPERATOR_STS_TLS_ENABLED"));
    assert!(k8s_deploy.contains("name: OPERATOR_STS_TLS_AUTO"));
    assert!(!k8s_deploy.contains("name: OPERATOR_STS_TLS_SECRET_NAME"));
    assert!(k8s_sts_svc.contains("name: rustfs-operator-sts"));
    assert!(k8s_sts_svc.contains("targetPort: sts"));

    // Ensure k8s dev manifests stay valid YAML after additions.
    let rbac_documents = yaml_documents(&k8s_rbac, "operator-rbac");
    assert_reference_cluster_role_is_read_only(&rbac_documents, "rustfs-operator");
    assert_sts_tls_role_is_minimal(
        &rbac_documents,
        "rustfs-operator-sts-tls",
        "rustfs-system",
        "rustfs-operator",
    );
    assert_yaml_documents_parse(&k8s_deploy, "operator-deployment");
    assert_yaml_documents_parse(&k8s_sts_svc, "operator-sts-service");
}

#[test]
fn helm_sts_template_and_values_are_consistent() {
    let helm_values = std::fs::read_to_string("../deploy/rustfs-operator/values.yaml")
        .expect("helm values exists");
    let helm_deploy =
        std::fs::read_to_string("../deploy/rustfs-operator/templates/deployment.yaml")
            .expect("helm deployment template exists");
    let helm_sts_svc =
        std::fs::read_to_string("../deploy/rustfs-operator/templates/operator-sts-service.yaml")
            .expect("helm sts service template exists");
    let helm_clusterrole =
        std::fs::read_to_string("../deploy/rustfs-operator/templates/clusterrole.yaml")
            .expect("helm clusterrole template exists");
    let helm_sts_tls_role =
        std::fs::read_to_string("../deploy/rustfs-operator/templates/operator-sts-tls-role.yaml")
            .expect("helm STS TLS role template exists");

    let sts_values = helm_values
        .split("# ServiceAccount configuration")
        .next()
        .expect("values contain sts section before service account");
    assert!(sts_values.contains("sts:"));
    assert!(sts_values.contains("enabled: true"));
    assert!(sts_values.contains("audience: sts.rustfs.com"));
    assert!(sts_values.contains("port: 4223"));
    assert!(sts_values.contains("tls:"));
    assert!(!sts_values.contains("secretName:"));
    assert!(!sts_values.contains("nodePort:"));
    assert!(!sts_values.contains("loadBalancerIP:"));
    assert!(!helm_values.contains("OPERATOR_STS_PORT"));

    assert!(helm_deploy.contains("app.kubernetes.io/component: operator"));
    assert!(helm_deploy.contains("{{- if .Values.sts.enabled }}"));
    assert!(helm_deploy.contains("name: sts"));
    assert!(helm_deploy.contains("containerPort: {{ .Values.sts.port }}"));
    assert!(helm_deploy.contains("name: OPERATOR_STS_ENABLED"));
    assert!(helm_deploy.contains("value: {{ .Values.sts.enabled | quote }}"));
    assert!(helm_deploy.contains("name: OPERATOR_STS_AUDIENCE"));
    assert!(helm_deploy.contains("value: {{ .Values.sts.audience | quote }}"));
    assert!(helm_deploy.contains("name: OPERATOR_STS_PORT"));
    assert!(helm_deploy.contains("value: {{ .Values.sts.port | quote }}"));
    assert!(helm_deploy.contains("name: OPERATOR_NAMESPACE"));
    assert!(helm_deploy.contains("fieldPath: metadata.namespace"));
    assert!(helm_deploy.contains("name: OPERATOR_STS_SERVICE_NAME"));
    assert!(
        helm_deploy
            .contains("{{ printf \"%s-sts\" (include \"rustfs-operator.fullname\" .) | quote }}")
    );
    assert!(helm_deploy.contains("name: OPERATOR_STS_TLS_ENABLED"));
    assert!(helm_deploy.contains("value: {{ .Values.sts.tls.enabled | quote }}"));
    assert!(helm_deploy.contains("name: OPERATOR_STS_TLS_AUTO"));
    assert!(helm_deploy.contains("value: {{ .Values.sts.tls.auto | quote }}"));
    assert!(!helm_deploy.contains("name: OPERATOR_STS_TLS_SECRET_NAME"));
    for &(name, value) in RESERVED_OPERATOR_ENV {
        assert!(
            helm_deploy.contains(&format!("\"{name}\" \"{value}\"")),
            "deployment template must reserve {name} for {value}"
        );
    }

    assert!(helm_clusterrole.contains("policybindings"));
    assert!(helm_clusterrole.contains("tokenreviews"));
    assert!(helm_clusterrole.contains("resources: [\"configmaps\", \"secrets\"]"));
    assert!(helm_clusterrole.contains("verbs: [\"get\", \"list\", \"watch\"]"));
    assert!(helm_sts_tls_role.contains(
        "if and .Values.rbac.create .Values.sts.enabled .Values.sts.tls.enabled .Values.sts.tls.auto"
    ));
    assert!(helm_sts_tls_role.contains("resourceNames: [\"sts-tls\"]"));
    assert!(helm_sts_tls_role.contains("verbs: [\"get\", \"update\"]"));
    assert!(!helm_sts_tls_role.contains("\"patch\""));
    assert!(!helm_sts_tls_role.contains("\"delete\""));

    assert!(helm_sts_svc.contains("{{ include \"rustfs-operator.fullname\" . }}-sts"));
    assert!(helm_sts_svc.contains("targetPort: sts"));
    assert!(helm_sts_svc.contains("app.kubernetes.io/component: operator"));
    assert!(helm_sts_svc.contains("operator STS currently supports only ClusterIP"));
    assert!(!helm_sts_svc.contains("nodePort:"));
    assert!(!helm_sts_svc.contains("loadBalancerIP:"));

    // Static assertions keep the value/template contract visible even when helm is unavailable.
    assert!(helm_sts_svc.contains("{{- if .Values.sts.enabled -}}"));
}

#[test]
fn helm_template_renders_sts_enabled_disabled_and_rejects_external_plaintext() {
    let Some(default_render) = helm_template(&[]) else {
        return;
    };

    assert!(
        default_render.status.success(),
        "default helm template should render successfully: {}",
        String::from_utf8_lossy(&default_render.stderr)
    );
    let default_stdout = String::from_utf8(default_render.stdout).expect("helm stdout is utf8");
    assert!(default_stdout.contains("name: rustfs-operator-sts"));
    assert!(default_stdout.contains("name: OPERATOR_STS_ENABLED"));
    assert!(default_stdout.contains("value: \"true\""));
    assert!(default_stdout.contains("name: OPERATOR_STS_AUDIENCE"));
    assert!(default_stdout.contains("value: \"sts.rustfs.com\""));
    assert!(default_stdout.contains("name: OPERATOR_STS_PORT"));
    assert!(default_stdout.contains("name: OPERATOR_STS_TLS_ENABLED"));
    assert!(default_stdout.contains("value: \"true\""));
    assert!(default_stdout.contains("name: OPERATOR_STS_TLS_AUTO"));
    assert!(!default_stdout.contains("name: OPERATOR_STS_TLS_SECRET_NAME"));
    for name in admission_env_names() {
        assert!(
            default_stdout.contains(&format!("name: {name}")),
            "default chart values must configure {name}"
        );
    }
    let default_documents = yaml_documents(&default_stdout, "helm-default-render");
    assert_reference_cluster_role_is_read_only(&default_documents, "rustfs-operator");
    assert_sts_tls_role_is_minimal(
        &default_documents,
        "rustfs-operator-sts-tls",
        "default",
        "rustfs-operator",
    );

    let Some(disabled_render) = helm_template(&["--set", "sts.enabled=false"]) else {
        return;
    };
    assert!(
        disabled_render.status.success(),
        "disabled helm template should render successfully: {}",
        String::from_utf8_lossy(&disabled_render.stderr)
    );
    let disabled_stdout =
        String::from_utf8(disabled_render.stdout).expect("disabled helm stdout is utf8");
    assert!(!disabled_stdout.contains("name: rustfs-operator-sts"));
    assert!(disabled_stdout.contains("name: OPERATOR_STS_ENABLED"));
    assert!(disabled_stdout.contains("value: \"false\""));
    assert!(!disabled_stdout.contains("name: OPERATOR_STS_PORT"));
    assert!(!disabled_stdout.contains("name: OPERATOR_STS_TLS_ENABLED"));
    assert_yaml_documents_parse(&disabled_stdout, "helm-disabled-render");
    assert!(
        find_document(
            &yaml_documents(&disabled_stdout, "helm-disabled-render"),
            "Role",
            "rustfs-operator-sts-tls",
        )
        .is_none(),
        "disabling STS must omit its namespaced Secret write role"
    );

    let Some(external_tls_render) = helm_template(&["--set", "sts.tls.auto=false"]) else {
        return;
    };
    assert!(
        external_tls_render.status.success(),
        "externally managed STS TLS should render successfully: {}",
        String::from_utf8_lossy(&external_tls_render.stderr)
    );
    let external_tls_stdout =
        String::from_utf8(external_tls_render.stdout).expect("external TLS stdout is utf8");
    let external_tls_documents = yaml_documents(&external_tls_stdout, "helm-external-tls-render");
    assert!(
        find_document(&external_tls_documents, "Role", "rustfs-operator-sts-tls",).is_none(),
        "externally managed STS TLS must not grant Secret write access"
    );
    assert_reference_cluster_role_is_read_only(&external_tls_documents, "rustfs-operator");

    for (setting, description) in [
        ("sts.tls.enabled=false", "disabled STS TLS"),
        ("rbac.create=false", "externally managed RBAC"),
    ] {
        let render = helm_template(&["--set", setting])
            .expect("helm was available for the preceding render");
        assert!(
            render.status.success(),
            "{description} should render successfully: {}",
            String::from_utf8_lossy(&render.stderr)
        );
        let stdout = String::from_utf8(render.stdout).expect("conditional render stdout is utf8");
        assert!(
            find_document(
                &yaml_documents(&stdout, description),
                "Role",
                "rustfs-operator-sts-tls",
            )
            .is_none(),
            "{description} must omit the STS TLS Secret write role"
        );
    }

    let custom_rbac_render = helm_template(&[
        "--namespace",
        "operators",
        "--set",
        "serviceAccount.create=false",
        "--set",
        "serviceAccount.name=custom-operator",
    ])
    .expect("helm was available for the preceding render");
    assert!(
        custom_rbac_render.status.success(),
        "custom STS RBAC render should succeed: {}",
        String::from_utf8_lossy(&custom_rbac_render.stderr)
    );
    let custom_rbac_stdout =
        String::from_utf8(custom_rbac_render.stdout).expect("custom RBAC stdout is utf8");
    assert_sts_tls_role_is_minimal(
        &yaml_documents(&custom_rbac_stdout, "helm-custom-rbac-render"),
        "rustfs-operator-sts-tls",
        "operators",
        "custom-operator",
    );

    for &(name, value) in RESERVED_OPERATOR_ENV {
        let name_override = format!("operator.env[0].name={name}");
        let render = helm_template(&[
            "--set",
            name_override.as_str(),
            "--set",
            "operator.env[0].value=invalid",
        ])
        .expect("helm was available for the preceding render");
        assert!(
            !render.status.success(),
            "chart-managed environment variable {name} must be rejected"
        );
        assert!(
            String::from_utf8_lossy(&render.stderr).contains(&format!(
                "operator.env must not set {name}; use {value} instead"
            )),
            "rejection for {name} must direct users to {value}"
        );
    }

    let Some(external_render) = helm_template(&["--set", "sts.service.type=NodePort"]) else {
        return;
    };
    assert!(
        !external_render.status.success(),
        "NodePort STS should fail until TLS termination is configured"
    );
    let external_stderr = String::from_utf8_lossy(&external_render.stderr);
    assert!(external_stderr.contains("operator STS currently supports only ClusterIP"));
}

#[test]
fn helm_template_renders_reused_values_without_admission_maps() {
    if !helm_is_available() {
        assert!(
            std::env::var_os("CI").is_none(),
            "helm must be installed in CI so chart render assertions cannot be skipped"
        );
        eprintln!("skipping helm template assertions: helm binary is not available");
        return;
    }

    let chart = chart_without_admission_defaults();
    let render = helm_template_chart(chart.path(), &[]);
    assert!(
        render.status.success(),
        "values reused from a release without admission maps must render successfully: {}",
        String::from_utf8_lossy(&render.stderr)
    );

    let stdout = String::from_utf8(render.stdout).expect("reused-values helm stdout is utf8");
    let documents = yaml_documents(&stdout, "helm-reused-values-render");
    assert!(
        find_document(&documents, "Deployment", "rustfs-operator").is_some(),
        "operator Deployment must still render"
    );
    assert!(
        find_document(&documents, "Deployment", "rustfs-operator-console").is_some(),
        "Console Deployment must still render"
    );
    for name in admission_env_names() {
        assert!(
            !stdout.contains(&format!("name: {name}")),
            "missing admission values must leave {name} unset so the binary default is used"
        );
    }
}

fn admission_env_names() -> [&'static str; 10] {
    [
        "OPERATOR_STS_ADMISSION_REQUESTS_PER_SECOND",
        "OPERATOR_STS_ADMISSION_BURST",
        "OPERATOR_STS_ADMISSION_MAX_IN_FLIGHT",
        "OPERATOR_STS_ADMISSION_BODY_LIMIT_BYTES",
        "OPERATOR_STS_ADMISSION_TIMEOUT_SECONDS",
        "CONSOLE_LOGIN_ADMISSION_REQUESTS_PER_SECOND",
        "CONSOLE_LOGIN_ADMISSION_BURST",
        "CONSOLE_LOGIN_ADMISSION_MAX_IN_FLIGHT",
        "CONSOLE_LOGIN_ADMISSION_BODY_LIMIT_BYTES",
        "CONSOLE_LOGIN_ADMISSION_TIMEOUT_SECONDS",
    ]
}

fn chart_without_admission_defaults() -> tempfile::TempDir {
    let chart = tempfile::tempdir().expect("temporary chart directory is created");
    copy_directory(Path::new("../deploy/rustfs-operator"), chart.path());

    let values_path = chart.path().join("values.yaml");
    let mut values: Value = serde_yaml_ng::from_str(
        &std::fs::read_to_string(&values_path).expect("temporary chart values exist"),
    )
    .expect("chart values parse");
    remove_mapping_key(&mut values, "sts", "admission");
    remove_mapping_key(&mut values, "console", "loginAdmission");
    std::fs::write(
        values_path,
        serde_yaml_ng::to_string(&values).expect("old release values serialize"),
    )
    .expect("old release values are written");
    chart
}

fn remove_mapping_key(values: &mut Value, section: &str, key: &str) {
    values[section]
        .as_mapping_mut()
        .unwrap_or_else(|| panic!("chart values must contain the {section} mapping"))
        .remove(Value::String(key.to_string()));
}

fn copy_directory(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).expect("temporary chart directory is created");
    for entry in std::fs::read_dir(source).expect("chart directory is readable") {
        let entry = entry.expect("chart entry is readable");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry
            .file_type()
            .expect("chart entry type is readable")
            .is_dir()
        {
            copy_directory(&source_path, &destination_path);
        } else {
            std::fs::copy(source_path, destination_path).expect("chart file is copied");
        }
    }
}

#[test]
fn console_session_deployments_enforce_single_recreate_process() {
    let dev_manifest = std::fs::read_to_string("../deploy/k8s-dev/console-deployment.yaml")
        .expect("k8s dev Console deployment exists");
    let dev_deployment = find_yaml_document(&dev_manifest, "Deployment", "rustfs-operator-console")
        .expect("k8s dev Console deployment is rendered");
    assert_eq!(dev_deployment["spec"]["replicas"].as_i64(), Some(1));
    assert_eq!(
        dev_deployment["spec"]["strategy"]["type"].as_str(),
        Some("Recreate")
    );
    assert_explicit_null_rolling_update(&dev_deployment);

    let Some(default_render) = helm_template(&[]) else {
        return;
    };
    assert!(
        default_render.status.success(),
        "default helm template should render successfully: {}",
        String::from_utf8_lossy(&default_render.stderr)
    );
    let default_stdout = String::from_utf8(default_render.stdout).expect("helm stdout is utf8");
    let deployment = find_yaml_document(&default_stdout, "Deployment", "rustfs-operator-console")
        .expect("Helm Console deployment is rendered");
    assert_eq!(deployment["spec"]["replicas"].as_i64(), Some(1));
    assert_eq!(
        deployment["spec"]["strategy"]["type"].as_str(),
        Some("Recreate")
    );
    assert_explicit_null_rolling_update(&deployment);

    for replicas in ["2", "true"] {
        let render = helm_template(&["--set", &format!("console.replicas={replicas}")])
            .expect("helm remains available");
        assert!(!render.status.success());
        assert!(
            String::from_utf8_lossy(&render.stderr).contains(
                "console.replicas must be 1 because Console sessions are stored in process"
            )
        );
    }

    let disabled_render = helm_template(&[
        "--set",
        "console.enabled=false",
        "--set",
        "console.replicas=2",
    ])
    .expect("helm remains available");
    assert!(disabled_render.status.success());
    let disabled_stdout =
        String::from_utf8(disabled_render.stdout).expect("disabled helm stdout is utf8");
    assert!(
        find_yaml_document(&disabled_stdout, "Deployment", "rustfs-operator-console").is_none()
    );
}

fn helm_template(args: &[&str]) -> Option<Output> {
    if !helm_is_available() {
        assert!(
            std::env::var_os("CI").is_none(),
            "helm must be installed in CI so chart render assertions cannot be skipped"
        );
        eprintln!("skipping helm template assertions: helm binary is not available");
        return None;
    }

    Some(helm_template_chart(
        Path::new("../deploy/rustfs-operator"),
        args,
    ))
}

fn helm_template_chart(chart: &Path, args: &[&str]) -> Output {
    let mut command = Command::new("helm");
    command.arg("template").arg("rustfs-operator").arg(chart);
    command.args(args);
    command.output().expect("helm template command runs")
}

fn helm_is_available() -> bool {
    Command::new("helm")
        .args(["version", "--short"])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn assert_yaml_documents_parse(yaml: &str, name: &str) {
    let _ = yaml_documents(yaml, name);
}

fn yaml_documents(yaml: &str, name: &str) -> Vec<Value> {
    let mut documents = Vec::new();

    for raw_doc in yaml.split("---") {
        if raw_doc.trim().is_empty() {
            continue;
        }

        let document = serde_yaml_ng::from_str::<Value>(raw_doc).unwrap_or_else(|error| {
            panic!("{name} contains invalid yaml document: {error}");
        });
        documents.push(document);
    }

    assert!(
        !documents.is_empty(),
        "{name} should contain at least one yaml document"
    );
    documents
}

fn find_document<'a>(documents: &'a [Value], kind: &str, name: &str) -> Option<&'a Value> {
    documents.iter().find(|document| {
        document["kind"].as_str() == Some(kind)
            && document["metadata"]["name"].as_str() == Some(name)
    })
}

fn assert_reference_cluster_role_is_read_only(documents: &[Value], name: &str) {
    let cluster_role = find_document(documents, "ClusterRole", name)
        .unwrap_or_else(|| panic!("missing ClusterRole {name}"));
    let rules = cluster_role["rules"]
        .as_sequence()
        .expect("ClusterRole rules must be a sequence");

    for resource in ["configmaps", "secrets"] {
        let matching = rules
            .iter()
            .filter(|rule| {
                let resources = yaml_strings(&rule["resources"]);
                resources.contains(resource) || resources.contains("*")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            matching.len(),
            1,
            "ClusterRole {name} must have exactly one rule for {resource}"
        );
        assert_eq!(
            yaml_strings(&matching[0]["verbs"]),
            BTreeSet::from(["get", "list", "watch"]),
            "ClusterRole {name} must keep {resource} read-only"
        );
        assert_eq!(
            yaml_strings(&matching[0]["apiGroups"]),
            BTreeSet::from([""]),
            "ClusterRole {name} must scope {resource} to the core API group"
        );
    }
}

fn assert_sts_tls_role_is_minimal(
    documents: &[Value],
    name: &str,
    namespace: &str,
    service_account: &str,
) {
    let role = find_document(documents, "Role", name)
        .unwrap_or_else(|| panic!("missing Role {namespace}/{name}"));
    assert_eq!(role["metadata"]["namespace"].as_str(), Some(namespace));
    let rules = role["rules"]
        .as_sequence()
        .expect("STS TLS Role rules must be a sequence");
    assert_eq!(rules.len(), 2);

    let create_rule = rules
        .iter()
        .find(|rule| yaml_strings(&rule["verbs"]) == BTreeSet::from(["create"]))
        .expect("STS TLS Role must contain the namespaced Secret create grant");
    assert_eq!(
        yaml_strings(&create_rule["apiGroups"]),
        BTreeSet::from([""])
    );
    assert_eq!(
        yaml_strings(&create_rule["resources"]),
        BTreeSet::from(["secrets"])
    );
    assert!(create_rule["resourceNames"].is_null());

    let update_rule = rules
        .iter()
        .find(|rule| yaml_strings(&rule["resourceNames"]) == BTreeSet::from(["sts-tls"]))
        .expect("STS TLS Role must scope updates to sts-tls");
    assert_eq!(
        yaml_strings(&update_rule["apiGroups"]),
        BTreeSet::from([""])
    );
    assert_eq!(
        yaml_strings(&update_rule["resources"]),
        BTreeSet::from(["secrets"])
    );
    assert_eq!(
        yaml_strings(&update_rule["verbs"]),
        BTreeSet::from(["get", "update"])
    );

    let binding = find_document(documents, "RoleBinding", name)
        .unwrap_or_else(|| panic!("missing RoleBinding {namespace}/{name}"));
    assert_eq!(binding["metadata"]["namespace"].as_str(), Some(namespace));
    assert_eq!(
        binding["roleRef"]["apiGroup"].as_str(),
        Some("rbac.authorization.k8s.io")
    );
    assert_eq!(binding["roleRef"]["kind"].as_str(), Some("Role"));
    assert_eq!(binding["roleRef"]["name"].as_str(), Some(name));
    let subjects = binding["subjects"]
        .as_sequence()
        .expect("STS TLS RoleBinding subjects must be a sequence");
    assert_eq!(subjects.len(), 1);
    assert_eq!(subjects[0]["kind"].as_str(), Some("ServiceAccount"));
    assert_eq!(subjects[0]["name"].as_str(), Some(service_account));
    assert_eq!(subjects[0]["namespace"].as_str(), Some(namespace));
}

fn yaml_strings(value: &Value) -> BTreeSet<&str> {
    value
        .as_sequence()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}

fn find_yaml_document(yaml: &str, kind: &str, name: &str) -> Option<Value> {
    yaml.split("---")
        .filter_map(|document| serde_yaml_ng::from_str::<Value>(document).ok())
        .find(|document| {
            document["kind"].as_str() == Some(kind)
                && document["metadata"]["name"].as_str() == Some(name)
        })
}

fn assert_explicit_null_rolling_update(deployment: &Value) {
    let strategy = deployment["spec"]["strategy"]
        .as_mapping()
        .expect("Deployment strategy is a mapping");
    let rolling_update = Value::String("rollingUpdate".to_string());
    assert!(strategy.contains_key(&rolling_update));
    assert!(strategy[&rolling_update].is_null());
}
