pub mod pool;
pub mod state;

use kube::KubeSchema;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub current_state: state::State,

    pub available_replicas: i32,

    pub pools: Vec<pool::Pool>,
}
