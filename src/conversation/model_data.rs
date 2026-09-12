use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ModelData {
    content: Map<String, Value>,
}

impl ModelData {
    #[allow(dead_code)]
    pub(crate) fn new(content: Map<String, Value>) -> Result<Self, InvalidModelData> {
        let model_data = Self { content };
        model_data.ensure_valid()?;
        Ok(model_data)
    }

    #[allow(dead_code)]
    pub(crate) fn content(&self) -> &Map<String, Value> {
        &self.content
    }

    pub(super) fn ensure_valid(&self) -> Result<(), InvalidModelData> {
        if self.content.is_empty() {
            return Err(InvalidModelData::EmptyContent);
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidModelData {
    EmptyContent,
}

impl Display for InvalidModelData {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyContent => write!(formatter, "model data content must not be empty"),
        }
    }
}

impl Error for InvalidModelData {}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    use super::{InvalidModelData, ModelData};
    fn content() -> Map<String, Value> {
        Map::from_iter([("response_id".to_owned(), Value::String("resp_1".to_owned()))])
    }

    #[test]
    fn model_data_round_trips_as_an_opaque_json_object() {
        let model_data = ModelData::new(content()).expect("the model data should be valid");

        let serialized =
            serde_json::to_value(&model_data).expect("the model data should serialize");
        let deserialized: ModelData =
            serde_json::from_value(serialized.clone()).expect("the model data should deserialize");

        assert_eq!(serialized, json!({ "response_id": "resp_1" }));
        assert_eq!(deserialized, model_data);
        assert_eq!(model_data.content(), &content());
    }

    #[test]
    fn model_data_rejects_empty_content() {
        assert_eq!(
            ModelData::new(Map::new()),
            Err(InvalidModelData::EmptyContent)
        );
    }
}
