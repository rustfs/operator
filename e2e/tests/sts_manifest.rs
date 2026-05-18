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
    assert_yaml_documents_parse(&k8s_rbac, "operator-rbac");
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

    assert!(helm_clusterrole.contains("policybindings"));
    assert!(helm_clusterrole.contains("tokenreviews"));

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
    assert_yaml_documents_parse(&default_stdout, "helm-default-render");

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
    let mut had_content = false;

    for raw_doc in yaml.split("---") {
        if raw_doc.trim().is_empty() {
            continue;
        }

        serde_yaml_ng::from_str::<Value>(raw_doc).unwrap_or_else(|error| {
            panic!("{name} contains invalid yaml document: {error}");
        });
        had_content = true;
    }

    assert!(
        had_content,
        "{name} should contain at least one yaml document"
    );
}
