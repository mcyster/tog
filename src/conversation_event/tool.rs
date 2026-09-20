use std::error::Error;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

use schemars::Schema;
use serde::de::Error as DeserializeError;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::model_data::InvalidModelData;
use super::{ModelData, ModelInvocationId, ToolCallId};

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

    #[allow(dead_code)]
    pub(crate) fn result(&self) -> &Schema {
        &self.result
    }

    pub(super) fn ensure_valid(&self) -> Result<(), InvalidToolData> {
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ToolRequest {
    call_id: ToolCallId,
    tool_name: ToolName,
    arguments: Value,
    invocation_id: ModelInvocationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data: Option<ModelData>,
}

impl ToolRequest {
    pub(crate) fn try_new(
        call_id: ToolCallId,
        tool_name: ToolName,
        arguments: Value,
        invocation_id: ModelInvocationId,
        data: Option<ModelData>,
    ) -> Result<Self, InvalidToolData> {
        let request = Self {
            call_id,
            tool_name,
            arguments,
            invocation_id,
            data,
        };
        request.ensure_valid()?;
        Ok(request)
    }

    pub(crate) fn call_id(&self) -> ToolCallId {
        self.call_id
    }

    pub(crate) fn tool_name(&self) -> &ToolName {
        &self.tool_name
    }

    pub(crate) fn arguments(&self) -> &Value {
        &self.arguments
    }

    #[allow(dead_code)]
    pub(crate) fn invocation_id(&self) -> ModelInvocationId {
        self.invocation_id
    }

    pub(crate) fn data(&self) -> Option<&ModelData> {
        self.data.as_ref()
    }

    pub(super) fn ensure_valid(&self) -> Result<(), InvalidToolData> {
        if let Some(data) = &self.data {
            data.ensure_valid().map_err(InvalidToolData::ModelData)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ToolResponse {
    call_id: ToolCallId,
    outcome: ToolOutcome,
}

impl ToolResponse {
    pub(crate) fn new(call_id: ToolCallId, outcome: ToolOutcome) -> Self {
        Self { call_id, outcome }
    }

    pub(crate) fn call_id(&self) -> ToolCallId {
        self.call_id
    }

    pub(crate) fn outcome(&self) -> &ToolOutcome {
        &self.outcome
    }

    pub(super) fn ensure_valid(&self) -> Result<(), InvalidToolData> {
        self.outcome.ensure_valid()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ToolOutcome {
    Result { value: Value },
    Problem { problem: ToolExecutionProblem },
}

impl ToolOutcome {
    fn ensure_valid(&self) -> Result<(), InvalidToolData> {
        match self {
            Self::Result { .. } => Ok(()),
            Self::Problem { problem } => problem.ensure_valid(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct ToolExecutionProblem {
    kind: ToolExecutionProblemKind,
    message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ToolExecutionProblemKind {
    InvalidArguments,
    UnknownTool,
    TimedOut,
    ExecutionFailed,
}

impl ToolExecutionProblem {
    pub(crate) fn try_new(
        kind: ToolExecutionProblemKind,
        message: String,
        details: Option<Value>,
    ) -> Result<Self, InvalidToolData> {
        let problem = Self {
            kind,
            message,
            details,
        };
        problem.ensure_valid()?;
        Ok(problem)
    }

    pub(crate) fn try_invalid_arguments(message: String) -> Result<Self, InvalidToolData> {
        Self::try_new(ToolExecutionProblemKind::InvalidArguments, message, None)
    }

    pub(crate) fn unknown_tool(tool_name: ToolName) -> Self {
        Self {
            kind: ToolExecutionProblemKind::UnknownTool,
            message: format!("unknown tool: {tool_name}"),
            details: None,
        }
    }

    pub(crate) fn try_execution_failed(message: String) -> Result<Self, InvalidToolData> {
        Self::try_new(ToolExecutionProblemKind::ExecutionFailed, message, None)
    }

    pub(crate) fn try_timed_out(
        message: String,
        details: Option<Value>,
    ) -> Result<Self, InvalidToolData> {
        Self::try_new(ToolExecutionProblemKind::TimedOut, message, details)
    }

    #[allow(dead_code)]
    pub(crate) fn kind(&self) -> ToolExecutionProblemKind {
        self.kind
    }

    #[allow(dead_code)]
    pub(crate) fn message(&self) -> &str {
        &self.message
    }

    #[allow(dead_code)]
    pub(crate) fn details(&self) -> Option<&Value> {
        self.details.as_ref()
    }

    fn ensure_valid(&self) -> Result<(), InvalidToolData> {
        if self.message.trim().is_empty() {
            return Err(InvalidToolData::EmptyProblemMessage);
        }
        Ok(())
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum InvalidToolData {
    EmptyToolName,
    EmptyDescription,
    EmptyProblemMessage,
    ModelData(InvalidModelData),
}

impl Display for InvalidToolData {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyToolName => write!(formatter, "tool name must not be empty"),
            Self::EmptyDescription => write!(formatter, "tool description must not be empty"),
            Self::EmptyProblemMessage => {
                write!(formatter, "tool problem message must not be empty")
            }
            Self::ModelData(error) => Display::fmt(error, formatter),
        }
    }
}

impl Error for InvalidToolData {}

#[cfg(test)]
mod tests {
    use schemars::json_schema;
    use serde_json::json;

    use super::{
        InvalidToolData, ToolDefinition, ToolExecutionProblem, ToolExecutionProblemKind, ToolName,
        ToolOutcome, ToolRequest, ToolResponse,
    };
    use crate::conversation_event::{ModelData, ModelInvocationId, ToolCallId};

    fn parameters_schema() -> schemars::Schema {
        json_schema!({ "type": "object", "properties": { "command": { "type": "string" } } })
    }

    fn definition(name: &str) -> ToolDefinition {
        ToolDefinition::try_new(
            ToolName::try_new(name.to_owned()).expect("the tool name should be valid"),
            "Run something.".to_owned(),
            parameters_schema(),
            json_schema!({ "type": "object" }),
        )
        .expect("the tool definition should be valid")
    }

    #[test]
    fn tool_names_reject_blank_values() {
        assert_eq!(
            ToolName::try_new("   ".to_owned()),
            Err(InvalidToolData::EmptyToolName)
        );
        assert!(serde_json::from_str::<ToolName>("\"  \"").is_err());
    }

    #[test]
    fn tool_definitions_round_trip_with_both_schemas() {
        let tool_definition = definition("shell");

        let serialized =
            serde_json::to_value(&tool_definition).expect("the tool definition should serialize");
        let restored: ToolDefinition = serde_json::from_value(serialized.clone())
            .expect("the tool definition should deserialize");

        assert_eq!(
            serialized["name"], "shell",
            "the tool name should be part of the definition"
        );
        assert_eq!(
            serialized["description"], "Run something.",
            "the description should be recorded"
        );
        assert!(
            serialized["parameters"].is_object(),
            "the parameter schema should be recorded"
        );
        assert!(
            serialized["result"].is_object(),
            "the result schema should be recorded"
        );
        assert_eq!(restored, tool_definition);
        assert_eq!(tool_definition.name().as_str(), "shell");
        assert_eq!(tool_definition.parameters(), &parameters_schema());
    }

    #[test]
    fn tool_definitions_reject_a_blank_description() {
        assert_eq!(
            ToolDefinition::try_new(
                ToolName::try_new("shell".to_owned()).expect("the tool name should be valid"),
                "  ".to_owned(),
                parameters_schema(),
                json_schema!({ "type": "object" }),
            ),
            Err(InvalidToolData::EmptyDescription)
        );
    }

    #[test]
    fn tool_requests_and_responses_round_trip_with_correlation() {
        let call_id = ToolCallId::new();
        let request = ToolRequest::try_new(
            call_id,
            ToolName::try_new("shell".to_owned()).expect("the tool name should be valid"),
            json!({ "command": "pwd" }),
            ModelInvocationId::new(),
            Some(
                ModelData::new(
                    [("call_id".to_owned(), json!("call_provider"))]
                        .into_iter()
                        .collect(),
                )
                .expect("the model data should be valid"),
            ),
        )
        .expect("the tool request should be valid");
        let response = ToolResponse::new(
            call_id,
            ToolOutcome::Result {
                value: json!({ "stdout": "/tmp\n" }),
            },
        );

        let restored_request: ToolRequest =
            serde_json::from_value(serde_json::to_value(&request).expect("the request serializes"))
                .expect("the request should deserialize");
        let restored_response: ToolResponse = serde_json::from_value(
            serde_json::to_value(&response).expect("the response serializes"),
        )
        .expect("the response should deserialize");

        assert_eq!(restored_request, request);
        assert_eq!(restored_request.call_id(), call_id);
        assert_eq!(restored_request.tool_name().as_str(), "shell");
        assert_eq!(restored_request.arguments(), &json!({ "command": "pwd" }));
        assert_eq!(restored_response, response);
        assert_eq!(restored_response.call_id(), call_id);
    }

    #[test]
    fn tool_execution_problems_round_trip_kind_message_and_details() {
        let problem = ToolExecutionProblem::try_new(
            ToolExecutionProblemKind::ExecutionFailed,
            "the process could not be started".to_owned(),
            Some(json!({ "errno": 2 })),
        )
        .expect("the problem should be valid");

        let serialized = serde_json::to_value(&problem).expect("the problem should serialize");
        let restored: ToolExecutionProblem =
            serde_json::from_value(serialized.clone()).expect("the problem should deserialize");

        assert_eq!(
            serialized,
            json!({
                "kind": "execution_failed",
                "message": "the process could not be started",
                "details": { "errno": 2 }
            })
        );
        assert_eq!(restored, problem);
        assert_eq!(restored.kind(), ToolExecutionProblemKind::ExecutionFailed);
        assert_eq!(restored.message(), "the process could not be started");
        assert_eq!(restored.details(), Some(&json!({ "errno": 2 })));
    }

    #[test]
    fn a_shared_timeout_problem_is_valid_without_details() {
        let problem = ToolExecutionProblem::try_timed_out("the tool timed out".to_owned(), None)
            .expect("a timeout without details should be valid");

        let serialized = serde_json::to_value(&problem).expect("the problem should serialize");
        let restored: ToolExecutionProblem =
            serde_json::from_value(serialized.clone()).expect("the problem should deserialize");

        assert_eq!(problem.kind(), ToolExecutionProblemKind::TimedOut);
        assert!(problem.details().is_none());
        assert!(serialized.get("details").is_none());
        assert_eq!(restored, problem);
    }

    #[test]
    fn tool_problem_messages_preserve_valid_text() {
        let problem = ToolExecutionProblem::try_new(
            ToolExecutionProblemKind::InvalidArguments,
            "  missing command\n".to_owned(),
            None,
        )
        .expect("the problem should be valid");

        assert_eq!(problem.message(), "  missing command\n");
    }

    #[test]
    fn blank_tool_problem_messages_are_rejected() {
        assert_eq!(
            ToolExecutionProblem::try_invalid_arguments("  ".to_owned()),
            Err(InvalidToolData::EmptyProblemMessage)
        );
        assert_eq!(
            ToolExecutionProblem::try_execution_failed(String::new()),
            Err(InvalidToolData::EmptyProblemMessage)
        );
        assert_eq!(
            ToolExecutionProblem::try_new(
                ToolExecutionProblemKind::TimedOut,
                "\n".to_owned(),
                Some(json!({ "timeout_seconds": 5 })),
            ),
            Err(InvalidToolData::EmptyProblemMessage)
        );
    }
}
