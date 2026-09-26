mod definition;
mod set;
#[cfg(test)]
mod tests;

use schemars::Schema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolAvailability {
    Immediate,
    Discoverable,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct ToolName(String);

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ToolDefinition {
    name: ToolName,
    description: String,
    parameters: Schema,
    result: Schema,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ToolsetEntry {
    definition: ToolDefinition,
    availability: ToolAvailability,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct Toolset {
    entries: Vec<ToolsetEntry>,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidToolDefinition {
    EmptyToolName,
    EmptyDescription,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidToolset {
    Definition(InvalidToolDefinition),
}
