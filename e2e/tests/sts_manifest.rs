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
use std::collections::BTreeSet;
use std::process::{Command, Output};

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
    assert_sts_tls_role_is_minimal(&rbac_documents, "rustfs-operator-sts-tls", "rustfs-system");
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
    for (name, value) in [
        ("OPERATOR_CLUSTER_DOMAIN", "clusterDomain"),
        ("OPERATOR_NAMESPACE", "namespace"),
        ("OPERATOR_STS_ENABLED", "sts.enabled"),
        ("OPERATOR_STS_AUDIENCE", "sts.audience"),
        ("OPERATOR_STS_PORT", "sts.port"),
        (
            "OPERATOR_STS_SERVICE_NAME",
            "the generated STS Service name",
        ),
        ("OPERATOR_STS_TLS_ENABLED", "sts.tls.enabled"),
        ("OPERATOR_STS_TLS_AUTO", "sts.tls.auto"),
    ] {
        assert!(
            helm_deploy.contains(&format!("\"{name}\" \"{value}\"")),
            "deployment template must reserve {name} for {value}"
        );
    }

    assert!(helm_clusterrole.contains("policybindings"));
    assert!(helm_clusterrole.contains("tokenreviews"));
    assert!(helm_clusterrole.contains("resources: [\"configmaps\", \"secrets\"]"));
    assert!(helm_clusterrole.contains("verbs: [\"get\", \"list\", \"watch\"]"));
    assert!(helm_sts_tls_role.contains(".Values.sts.tls.auto"));
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
    let default_documents = yaml_documents(&default_stdout, "helm-default-render");
    assert_reference_cluster_role_is_read_only(&default_documents, "rustfs-operator");
    assert_sts_tls_role_is_minimal(&default_documents, "rustfs-operator-sts-tls", "default");

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

    for (name, value) in [
        ("OPERATOR_CLUSTER_DOMAIN", "clusterDomain"),
        ("OPERATOR_NAMESPACE", "namespace"),
        ("OPERATOR_STS_ENABLED", "sts.enabled"),
        ("OPERATOR_STS_AUDIENCE", "sts.audience"),
        ("OPERATOR_STS_PORT", "sts.port"),
        (
            "OPERATOR_STS_SERVICE_NAME",
            "the generated STS Service name",
        ),
        ("OPERATOR_STS_TLS_ENABLED", "sts.tls.enabled"),
        ("OPERATOR_STS_TLS_AUTO", "sts.tls.auto"),
    ] {
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

fn helm_template(args: &[&str]) -> Option<Output> {
    if !helm_is_available() {
        eprintln!("skipping helm template assertions: helm binary is not available");
        return None;
    }

    let mut command = Command::new("helm");
    command.args(["template", "rustfs-operator", "../deploy/rustfs-operator"]);
    command.args(args);

    Some(command.output().expect("helm template command runs"))
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
            .filter(|rule| yaml_strings(&rule["resources"]).contains(resource))
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
    }
}

fn assert_sts_tls_role_is_minimal(documents: &[Value], name: &str, namespace: &str) {
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
        yaml_strings(&create_rule["resources"]),
        BTreeSet::from(["secrets"])
    );
    assert!(create_rule["resourceNames"].is_null());

    let update_rule = rules
        .iter()
        .find(|rule| yaml_strings(&rule["resourceNames"]) == BTreeSet::from(["sts-tls"]))
        .expect("STS TLS Role must scope updates to sts-tls");
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
    assert_eq!(binding["roleRef"]["kind"].as_str(), Some("Role"));
    assert_eq!(binding["roleRef"]["name"].as_str(), Some(name));
}

fn yaml_strings(value: &Value) -> BTreeSet<&str> {
    value
        .as_sequence()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect()
}
