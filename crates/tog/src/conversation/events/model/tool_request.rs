use std::error::Error;
use std::fmt::{Display, Formatter};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::conversation::events::ConversationEventId;
use crate::conversation::events::tools::ToolName;

use super::{InvalidModelData, ModelData};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ToolRequest {
    model_request_id: ConversationEventId,
    tool_name: ToolName,
    arguments: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data: Option<ModelData>,
}

impl ToolRequest {
    pub fn try_new(
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

    pub fn model_request_id(&self) -> ConversationEventId {
        self.model_request_id
    }

    pub fn tool_name(&self) -> &ToolName {
        &self.tool_name
    }

    pub fn arguments(&self) -> &Value {
        &self.arguments
    }

    pub fn data(&self) -> Option<&ModelData> {
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
pub enum InvalidToolRequest {
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
