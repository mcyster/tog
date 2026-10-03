use std::error::Error;
use std::fmt::{Display, Formatter};

use super::{InvalidTools, Tool, ToolAvailability, ToolDefinition, Tools};

impl Tool {
    pub fn new(definition: ToolDefinition, availability: ToolAvailability) -> Self {
        Self {
            definition,
            availability,
        }
    }

    pub fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    pub fn availability(&self) -> ToolAvailability {
        self.availability
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidTools> {
        self.definition
            .ensure_valid()
            .map_err(InvalidTools::Definition)
    }
}

impl Tools {
    pub fn new(tools: Vec<Tool>) -> Self {
        Self { tools }
    }

    pub fn tools(&self) -> &[Tool] {
        &self.tools
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidTools> {
        for tool in &self.tools {
            tool.ensure_valid()?;
        }
        Ok(())
    }
}

impl Display for InvalidTools {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Definition(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for InvalidTools {}
