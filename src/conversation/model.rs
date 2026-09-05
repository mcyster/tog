use std::error::Error;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use serde::de::Error as DeserializeError;
use serde::{Deserialize, Deserializer, Serialize};

use super::{InvalidModelData, ModelData};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ModelDetails {
    source: ModelSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data: Option<ModelData>,
}

impl ModelDetails {
    pub(crate) fn new(
        source: ModelSource,
        data: Option<ModelData>,
    ) -> Result<Self, InvalidModelData> {
        let model_details = Self { source, data };
        model_details.ensure_valid()?;
        Ok(model_details)
    }

    #[allow(dead_code)]
    pub(crate) fn source(&self) -> &ModelSource {
        &self.source
    }

    #[allow(dead_code)]
    pub(crate) fn data(&self) -> Option<&ModelData> {
        self.data.as_ref()
    }

    pub(super) fn ensure_valid(&self) -> Result<(), InvalidModelData> {
        if let Some(data) = &self.data {
            data.ensure_valid()?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ModelSource {
    provider: ProviderId,
    model: ModelId,
}

impl ModelSource {
    pub(crate) fn new(provider: ProviderId, model: ModelId) -> Self {
        Self { provider, model }
    }

    pub(crate) fn model(&self) -> &ModelId {
        &self.model
    }

    #[allow(dead_code)]
    pub(crate) fn provider(&self) -> &ProviderId {
        &self.provider
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ProviderId(String);

impl ProviderId {
    #[allow(dead_code)]
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for ProviderId {
    type Err = InvalidProviderId;

    fn from_str(unvalidated_value: &str) -> Result<Self, Self::Err> {
        let normalized_value = unvalidated_value.trim();
        if normalized_value.is_empty() {
            return Err(InvalidProviderId);
        }
        Ok(Self(normalized_value.to_owned()))
    }
}

impl<'de> Deserialize<'de> for ProviderId {
    fn deserialize<DeserializerType>(
        deserializer: DeserializerType,
    ) -> Result<Self, DeserializerType::Error>
    where
        DeserializerType: Deserializer<'de>,
    {
        let unvalidated_value = String::deserialize(deserializer)?;
        Self::from_str(&unvalidated_value).map_err(DeserializerType::Error::custom)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct InvalidProviderId;

impl Display for InvalidProviderId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "provider identifier must not be empty")
    }
}

impl Error for InvalidProviderId {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ModelId(String);

impl ModelId {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for ModelId {
    type Err = InvalidModelId;

    fn from_str(unvalidated_value: &str) -> Result<Self, Self::Err> {
        let normalized_value = unvalidated_value.trim();
        if normalized_value.is_empty() {
            return Err(InvalidModelId);
        }
        Ok(Self(normalized_value.to_owned()))
    }
}

impl<'de> Deserialize<'de> for ModelId {
    fn deserialize<DeserializerType>(
        deserializer: DeserializerType,
    ) -> Result<Self, DeserializerType::Error>
    where
        DeserializerType: Deserializer<'de>,
    {
        let unvalidated_value = String::deserialize(deserializer)?;
        Self::from_str(&unvalidated_value).map_err(DeserializerType::Error::custom)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct InvalidModelId;

impl Display for InvalidModelId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "model identifier must not be empty")
    }
}

impl Error for InvalidModelId {}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::{InvalidModelId, InvalidProviderId, ModelId, ModelSource, ProviderId};

    #[test]
    fn model_source_identifiers_reject_empty_values() {
        assert_eq!(ProviderId::from_str("  "), Err(InvalidProviderId));
        assert_eq!(ModelId::from_str("  "), Err(InvalidModelId));
        assert!(serde_json::from_str::<ProviderId>("\" \"").is_err());
        assert!(serde_json::from_str::<ModelId>("\" \"").is_err());
    }

    #[test]
    fn model_source_normalizes_identifiers_and_round_trips_through_json() {
        let source = ModelSource::new(
            ProviderId::from_str(" openai ").expect("the provider identifier should be valid"),
            ModelId::from_str(" gpt-5.6 ").expect("the model identifier should be valid"),
        );

        let json = serde_json::to_value(&source).expect("the model source should serialize");
        let deserialized_source: ModelSource =
            serde_json::from_value(json.clone()).expect("the model source should deserialize");

        assert_eq!(
            json,
            serde_json::json!({ "provider": "openai", "model": "gpt-5.6" })
        );
        assert_eq!(deserialized_source, source);
        assert_eq!(source.provider().as_str(), "openai");
        assert_eq!(source.model().as_str(), "gpt-5.6");
    }
}
