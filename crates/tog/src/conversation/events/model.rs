mod assistant_response;
mod request;
mod response;
mod specific;
mod tool_request;

pub use assistant_response::{AssistantResponse, InvalidAssistantResponse};
pub use request::{InvalidModelRequest, ModelRequest};
pub use response::{InvalidModelResponse, ModelOutcome, ModelResponse, Usage};
pub use specific::{InvalidModelSpecificEvent, ModelSpecificEvent};
pub use tool_request::{InvalidToolRequest, ToolRequest};

use std::error::Error;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use serde::de::Error as DeserializeError;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};

use super::ConversationEventId;

pub trait ModelEvent {
    fn model_request_id(&self) -> ConversationEventId;
}

impl ModelEvent for AssistantResponse {
    fn model_request_id(&self) -> ConversationEventId {
        self.model_request_id()
    }
}

impl ModelEvent for ToolRequest {
    fn model_request_id(&self) -> ConversationEventId {
        self.model_request_id()
    }
}

impl ModelEvent for ModelResponse {
    fn model_request_id(&self) -> ConversationEventId {
        self.model_request_id()
    }
}

impl ModelEvent for ModelSpecificEvent {
    fn model_request_id(&self) -> ConversationEventId {
        self.model_request_id()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ModelSource {
    provider: ProviderId,
    model: ModelId,
}

impl ModelSource {
    pub fn new(provider: ProviderId, model: ModelId) -> Self {
        Self { provider, model }
    }

    pub fn model(&self) -> &ModelId {
        &self.model
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ProviderId(String);

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
pub struct InvalidProviderId;

impl Display for InvalidProviderId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "provider identifier must not be empty")
    }
}

impl Error for InvalidProviderId {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ModelId(String);

impl ModelId {
    pub fn as_str(&self) -> &str {
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
pub struct InvalidModelId;

impl Display for InvalidModelId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "model identifier must not be empty")
    }
}

impl Error for InvalidModelId {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ModelData {
    content: Map<String, Value>,
}

impl ModelData {
    pub fn new(content: Map<String, Value>) -> Result<Self, InvalidModelData> {
        let model_data = Self { content };
        model_data.ensure_valid()?;
        Ok(model_data)
    }

    pub fn content(&self) -> &Map<String, Value> {
        &self.content
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidModelData> {
        if self.content.is_empty() {
            return Err(InvalidModelData::EmptyContent);
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum InvalidModelData {
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
