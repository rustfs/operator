use kube::KubeSchema;
use schemars::schema::Schema;
use schemars::{JsonSchema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use strum::Display;

#[derive(Deserialize, Serialize, Clone, Debug, KubeSchema)]
#[serde(rename_all = "camelCase")]
pub struct Pool {
    pub ss_name: String,
    pub state: PoolState,
}

#[derive(Deserialize, Serialize, Clone, Debug, Display)]
pub enum PoolState {
    #[strum(serialize = "PoolCreated")]
    Created,

    #[strum(serialize = "PoolNotCreated")]
    NotCreated,

    #[strum(serialize = "PoolInitialized")]
    Initialized,
}

impl JsonSchema for PoolState {
    fn schema_name() -> String {
        "State".to_owned()
    }
    fn schema_id() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed(concat!(module_path!(), "::", "State"))
    }
    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        Schema::Object(schemars::schema::SchemaObject {
            instance_type: Some(schemars::schema::InstanceType::String.into()),
            ..Default::default()
        })
    }
}
