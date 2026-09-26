use std::error::Error;
use std::fmt::{Display, Formatter};

use super::{InvalidToolset, ToolAvailability, ToolDefinition, Toolset, ToolsetEntry};

impl ToolsetEntry {
    pub(crate) fn new(definition: ToolDefinition, availability: ToolAvailability) -> Self {
        Self {
            definition,
            availability,
        }
    }

    pub(crate) fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    pub(crate) fn availability(&self) -> ToolAvailability {
        self.availability
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidToolset> {
        self.definition
            .ensure_valid()
            .map_err(InvalidToolset::Definition)
    }
}

impl Toolset {
    pub(crate) fn new(entries: Vec<ToolsetEntry>) -> Result<Self, InvalidToolset> {
        let toolset = Self { entries };
        toolset.ensure_valid()?;
        Ok(toolset)
    }

    pub(crate) fn immediate(definitions: Vec<ToolDefinition>) -> Result<Self, InvalidToolset> {
        let entries = definitions
            .into_iter()
            .map(|definition| ToolsetEntry::new(definition, ToolAvailability::Immediate))
            .collect();
        Self::new(entries)
    }

    pub(crate) fn entries(&self) -> &[ToolsetEntry] {
        &self.entries
    }

    pub(crate) fn ensure_valid(&self) -> Result<(), InvalidToolset> {
        for entry in &self.entries {
            entry.ensure_valid()?;
        }
        Ok(())
    }
}

impl Display for InvalidToolset {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Definition(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for InvalidToolset {}
