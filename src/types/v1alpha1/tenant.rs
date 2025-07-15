use crate::types::v1alpha1::pool::Pool;
use crate::types::v1alpha1::status::Status;
use k8s_openapi::schemars;
use kube::runtime::reflector::Lookup;
use kube::{CustomResource, KubeSchema};
use serde::{Deserialize, Serialize};

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, KubeSchema, Default)]
#[kube(
    group = "rustfs.com",
    version = "v1alpha1",
    kind = "Tenant",
    namespaced,
    status = "Status",
    shortname = "tenant",
    plural = "tenants",
    singular = "tenant",
    printcolumn = r#"{"name":"State", "type":"string", "jsonPath":".status.currentState"}"#,
    printcolumn = r#"{"name":"Health", "type":"string", "jsonPath":".status.healthStatus"}"#,
    printcolumn = r#"{"name":"Age", "type":"date", "jsonPath":".metadata.creationTimestamp"}"#,
    crates(serde_json = "k8s_openapi::serde_json")
)]
#[serde(rename_all = "camelCase")]
pub struct TenantSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduler: Option<String>,

    pub pools: Vec<Pool>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub image_pull_secret: Option<corev1::LocalObjectReference>,
    //
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub pod_management_policy: Option<String>,
    //
    // #[serde(default, skip_serializing_if = "Vec::is_empty")]
    // pub env: Vec<corev1::EnvVar>,
    //
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub mount_path: Option<String>,
    //
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub sub_path: Option<String>,
    //
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub request_auto_cert: Option<bool>,
    //
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub cert_expiry_alert_threshold: Option<i32>,
    //
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub liveness: Option<corev1::Probe>,
    //
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub readiness: Option<corev1::Probe>,
    //
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub startup: Option<corev1::Probe>,
    //
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub lifecycle: Option<corev1::Lifecycle>,

    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // features: Option<corev1::Lifecycle>,

    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // cert_config: Option<corev1::Lifecycle>,

    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // kes: Option<corev1::Lifecycle>,

    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // prometheus_operator: Option<corev1::Lifecycle>,

    // #[serde(default, skip_serializing_if = "Vec::is_empty")]
    // prometheus_operator_scrape_metrics_paths: Vec<String>,
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub service_account_name: Option<String>,
    //
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub priority_class_name: Option<String>,
    //
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub image_pull_policy: Option<String>,
    //
    // // #[serde(default, skip_serializing_if = "Option::is_none")]
    // // pub side_cars: Option<SideCars>,
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    // pub configuration: Option<corev1::TypedLocalObjectReference>,
}

impl Tenant {
    pub fn service_name(&self) -> String {
        format!("{}-hl", self.name().unwrap_or_default())
    }
}
