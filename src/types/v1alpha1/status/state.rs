use schemars::schema::Schema;
use schemars::{JsonSchema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use strum::Display;

#[derive(Deserialize, Serialize, Clone, Debug, Display)]
pub enum State {
    #[strum(serialize = "Initialized")]
    Initialized,

    #[strum(serialize = "Statefulset not controlled by operator")]
    NotOwned,
}

impl JsonSchema for State {
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
