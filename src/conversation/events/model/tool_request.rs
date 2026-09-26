use std::error::Error;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use schemars::Schema;
use serde::de::Error as DeserializeError;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::{InvalidModelData, ModelData};
use crate::conversation::events::ConversationEventId;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ToolRequest {
    model_request_id: ConversationEventId,
    tool_name: ToolName,
    arguments: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data: Option<ModelData>,
}

impl ToolRequest {
    pub(crate) fn try_new(
        model_request_id: ConversationEventId,
        tool_name: ToolName,
        arguments: Value,
        data: Option<ModelData>,
    ) -> Result<Self, InvalidToolRequest> {
        let request = Self {
            model_request_id,
            tool_name,
            arguments,
            data,
        };
        request.ensure_valid()?;
        Ok(request)
    }

    pub(crate) fn model_request_id(&self) -> ConversationEventId {
        self.model_request_id
    }

    pub(crate) fn tool_name(&self) -> &ToolName {
        &self.tool_name
    }

    pub(crate) fn arguments(&self) -> &Value {
        &self.arguments
    }

    pub(crate) fn data(&self) -> Option<&ModelData> {
        self.data.as_ref()
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidToolRequest> {
        if let Some(data) = &self.data {
            data.ensure_valid().map_err(InvalidToolRequest::ModelData)?;
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidToolRequest {
    ModelData(InvalidModelData),
}

impl Display for InvalidToolRequest {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModelData(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for InvalidToolRequest {}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ToolDefinition {
    name: ToolName,
    description: String,
    parameters: Schema,
    result: Schema,
}

impl ToolDefinition {
    pub(crate) fn try_new(
        name: ToolName,
        description: String,
        parameters: Schema,
        result: Schema,
    ) -> Result<Self, InvalidToolData> {
        let definition = Self {
            name,
            description,
            parameters,
            result,
        };
        definition.ensure_valid()?;
        Ok(definition)
    }

    pub(crate) fn name(&self) -> &ToolName {
        &self.name
    }

    pub(crate) fn description(&self) -> &str {
        &self.description
    }

    pub(crate) fn parameters(&self) -> &Schema {
        &self.parameters
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidToolData> {
        if self.description.trim().is_empty() {
            return Err(InvalidToolData::EmptyDescription);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ToolName(String);

impl ToolName {
    pub(crate) fn try_new(unvalidated_name: String) -> Result<Self, InvalidToolData> {
        let normalized_name = unvalidated_name.trim();
        if normalized_name.is_empty() {
            return Err(InvalidToolData::EmptyToolName);
        }
        Ok(Self(normalized_name.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for ToolName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl FromStr for ToolName {
    type Err = InvalidToolData;

    fn from_str(unvalidated_name: &str) -> Result<Self, Self::Err> {
        Self::try_new(unvalidated_name.to_owned())
    }
}

impl<'de> Deserialize<'de> for ToolName {
    fn deserialize<DeserializerType>(
        deserializer: DeserializerType,
    ) -> Result<Self, DeserializerType::Error>
    where
        DeserializerType: Deserializer<'de>,
    {
        let unvalidated_name = String::deserialize(deserializer)?;
        Self::try_new(unvalidated_name).map_err(DeserializerType::Error::custom)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidToolData {
    EmptyToolName,
    EmptyDescription,
}

impl Display for InvalidToolData {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyToolName => write!(formatter, "tool name must not be empty"),
            Self::EmptyDescription => write!(formatter, "tool description must not be empty"),
        }
    }
}

impl Error for InvalidToolData {}
