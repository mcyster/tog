use super::Toolset;
use tog::conversation::{Tool, ToolAvailability, ToolDefinition, Tools};

impl Toolset {
    pub fn immediate(definitions: Vec<ToolDefinition>) -> Self {
        Self {
            tools: definitions
                .into_iter()
                .map(|definition| Tool::new(definition, ToolAvailability::Immediate))
                .collect(),
        }
    }

    pub fn into_tools(self) -> Tools {
        Tools::new(self.tools)
    }
}
