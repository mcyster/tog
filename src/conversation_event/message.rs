use std::error::Error;
use std::fmt::{Display, Formatter};

#[cfg(test)]
use super::ModelEvent;
use super::{
    AssistantResponse, ConversationMessage, InvalidAssistantResponse, InvalidConversationEventKind,
    InvalidModelCommunication, ModelCommunication, ModelEventImportance,
};

impl ConversationMessage {
    pub(super) fn ensure_valid(&self) -> Result<(), InvalidConversationEventKind> {
        match self {
            Self::User { .. } => Ok(()),
            Self::AssistantResponse { data, response, .. } => {
                if let Some(data) = data {
                    data.ensure_valid()
                        .map_err(InvalidConversationEventKind::ModelData)?;
                }
                response
                    .ensure_valid()
                    .map_err(InvalidConversationEventKind::Assistant)
            }
            Self::Communication {
                data,
                communication,
                ..
            } => {
                if let Some(data) = data {
                    data.ensure_valid()
                        .map_err(InvalidConversationEventKind::ModelData)?;
                }
                communication
                    .ensure_valid()
                    .map_err(InvalidConversationEventKind::ModelCommunication)
            }
            Self::Problem { data, problem, .. } => {
                if let Some(data) = data {
                    data.ensure_valid()
                        .map_err(InvalidConversationEventKind::ModelData)?;
                }
                problem
                    .ensure_valid()
                    .map_err(InvalidConversationEventKind::ConversationProblem)
            }
        }
    }
}

impl AssistantResponse {
    pub(crate) fn new(message: String) -> Result<Self, InvalidAssistantResponse> {
        let response = Self { message };
        response.ensure_valid()?;
        Ok(response)
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    fn ensure_valid(&self) -> Result<(), InvalidAssistantResponse> {
        if self.message.trim().is_empty() {
            return Err(InvalidAssistantResponse::EmptyMessage);
        }
        Ok(())
    }
}

impl ModelCommunication {
    pub(crate) fn new(
        message: String,
        importance: ModelEventImportance,
        subtype: String,
    ) -> Result<Self, InvalidModelCommunication> {
        let communication = Self {
            message,
            importance,
            subtype,
        };
        communication.ensure_valid()?;
        Ok(communication)
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    pub(crate) fn importance(&self) -> ModelEventImportance {
        self.importance
    }

    pub(crate) fn subtype(&self) -> &str {
        &self.subtype
    }

    fn ensure_valid(&self) -> Result<(), InvalidModelCommunication> {
        if self.message.trim().is_empty() {
            return Err(InvalidModelCommunication::EmptyMessage);
        }
        if self.subtype().trim().is_empty() {
            return Err(InvalidModelCommunication::EmptySubtype);
        }
        Ok(())
    }
}

impl Display for InvalidAssistantResponse {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyMessage => write!(formatter, "assistant response message must not be empty"),
        }
    }
}

impl Error for InvalidAssistantResponse {}

impl Display for InvalidModelCommunication {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyMessage => {
                write!(formatter, "model communication message must not be empty")
            }
            Self::EmptySubtype => {
                write!(formatter, "model communication subtype must not be empty")
            }
        }
    }
}

impl Error for InvalidModelCommunication {}

#[cfg(test)]
impl ModelEvent {
    pub(crate) fn message(&self) -> &str {
        match self {
            Self::Assistant(response) => response.message(),
            Self::Communication(communication) => communication.message(),
        }
    }

    pub(crate) fn importance(&self) -> ModelEventImportance {
        match self {
            Self::Assistant(_) => ModelEventImportance::Important,
            Self::Communication(communication) => communication.importance(),
        }
    }
}
