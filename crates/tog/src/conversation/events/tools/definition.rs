use std::error::Error;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use schemars::Schema;
use serde::de::Error as DeserializeError;
use serde::{Deserialize, Deserializer};

use super::{InvalidToolDefinition, ToolDefinition, ToolName};

impl ToolName {
    pub fn try_new(unvalidated_name: String) -> Result<Self, InvalidToolDefinition> {
        let normalized_name = unvalidated_name.trim();
        if normalized_name.is_empty() {
            return Err(InvalidToolDefinition::EmptyToolName);
        }
        Ok(Self(normalized_name.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Display for ToolName {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl FromStr for ToolName {
    type Err = InvalidToolDefinition;

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

impl ToolDefinition {
    pub fn try_new(
        name: ToolName,
        description: String,
        parameters: Schema,
        result: Schema,
    ) -> Result<Self, InvalidToolDefinition> {
        let definition = Self {
            name,
            description,
            parameters,
            result,
        };
        definition.ensure_valid()?;
        Ok(definition)
    }

    pub fn name(&self) -> &ToolName {
        &self.name
    }

    pub fn description(&self) -> &str {
        &self.description
    }

    pub fn parameters(&self) -> &Schema {
        &self.parameters
    }

    #[allow(dead_code)]
    pub fn result(&self) -> &Schema {
        &self.result
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidToolDefinition> {
        if self.description.trim().is_empty() {
            return Err(InvalidToolDefinition::EmptyDescription);
        }
        Ok(())
    }
}

impl Display for InvalidToolDefinition {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyToolName => write!(formatter, "tool name must not be empty"),
            Self::EmptyDescription => write!(formatter, "tool description must not be empty"),
        }
    }
}

impl Error for InvalidToolDefinition {}
