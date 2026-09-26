use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};

use super::model::{InvalidToolData, ToolDefinition};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Context {
    ToolsAvailable { tools: Vec<ToolDefinition> },
}

impl Context {
    pub(crate) fn tools_available(tools: Vec<ToolDefinition>) -> Self {
        Self::ToolsAvailable { tools }
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidContext> {
        match self {
            Self::ToolsAvailable { tools } => {
                for tool in tools {
                    tool.ensure_valid().map_err(InvalidContext::Tool)?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidContext {
    Tool(InvalidToolData),
}

impl Display for InvalidContext {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tool(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for InvalidContext {}
