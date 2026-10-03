mod definition;
mod set;
#[cfg(test)]
mod tests;

use schemars::Schema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolAvailability {
    Immediate,
    Discoverable,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ToolName(String);

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolDefinition {
    name: ToolName,
    description: String,
    parameters: Schema,
    result: Schema,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Tool {
    definition: ToolDefinition,
    availability: ToolAvailability,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Tools {
    tools: Vec<Tool>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum InvalidToolDefinition {
    EmptyToolName,
    EmptyDescription,
}

#[derive(Debug, Eq, PartialEq)]
pub enum InvalidTools {
    Definition(InvalidToolDefinition),
}
