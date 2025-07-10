use k8s_openapi::api::core::v1 as corev1;
use kube::KubeSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema)]
#[serde(rename_all = "camelCase")]
pub struct Pool {
    pub name: String,
    pub servers: u64,
    pub volumes_ser_server: u64,
    pub volume_chain_template: corev1::PersistentVolumeClaim,
    pub path: String,
}
