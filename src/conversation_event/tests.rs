use serde_json::json;

use super::{
    AssistantResponse, ConversationCommand, ConversationCommandId, ConversationEventClass,
    ConversationEventEnvelope, ConversationEventKind, ConversationFact, ConversationLifecycle,
    ConversationMessage, ConversationProblem, ConversationTurnId, InvalidAssistantResponse,
    InvalidModelCommunication, ModelCommunication, ModelEventImportance, ModelInvocationId,
    ModelIssue, TurnOutcome,
};

#[test]
fn assistant_is_a_top_level_event_with_model_provenance() {
    let event = ConversationEventKind::Fact(ConversationFact::Message {
        message: ConversationMessage::AssistantResponse {
            invocation_id: ModelInvocationId::new(),
            data: None,
            response: AssistantResponse::new("The answer is 42.".to_owned())
                .expect("the assistant response should be valid"),
        },
        turn_id: Some(ConversationTurnId::new()),
    });

    let serialized = serde_json::to_value(&event).expect("the event should serialize");
    assert_eq!(serialized["class"], "fact");
    assert_eq!(serialized["event"]["type"], "assistant");
}

#[test]
fn problem_can_have_model_provenance_without_being_a_model_event() {
    let event = ConversationEventKind::Fact(ConversationFact::Message {
        message: ConversationMessage::Problem {
            invocation_id: Some(ModelInvocationId::new()),
            data: None,
            problem: ConversationProblem::Issue(
                ModelIssue::try_refusal("I cannot comply.".to_owned())
                    .expect("the refusal should be valid"),
            ),
        },
        turn_id: Some(ConversationTurnId::new()),
    });

    let serialized = serde_json::to_value(&event).expect("the problem should serialize");
    assert_eq!(serialized["class"], "fact");
    assert_eq!(serialized["event"]["type"], "problem");
}

#[test]
fn conversation_fact_reuses_the_message_payload_with_turn_association() {
    let turn_id = ConversationTurnId::new();
    let event = ConversationEventKind::Fact(ConversationFact::Message {
        message: ConversationMessage::AssistantResponse {
            invocation_id: ModelInvocationId::new(),
            data: None,
            response: AssistantResponse::new("Hello.".to_owned())
                .expect("the assistant response should be valid"),
        },
        turn_id: Some(turn_id),
    });

    let serialized = serde_json::to_value(&event).expect("the event should serialize");
    assert_eq!(serialized["event"]["type"], "assistant");
    assert_eq!(
        serialized["event"]["turn_id"],
        serde_json::to_value(turn_id).expect("the turn identifier should serialize")
    );

    let restored: ConversationEventKind =
        serde_json::from_value(serialized).expect("the event should deserialize");
    assert_eq!(restored, event);
}

#[test]
fn commands_and_turn_completion_are_distinct_from_model_output() {
    let command = ConversationEventKind::Command(ConversationCommand::TurnRequested {
        command_id: ConversationCommandId::new(),
        turn_id: ConversationTurnId::new(),
    });
    let completed = ConversationEventKind::Fact(ConversationFact::Lifecycle(
        ConversationLifecycle::TurnCompleted {
            turn_id: ConversationTurnId::new(),
            outcome: TurnOutcome::Succeeded,
        },
    ));

    assert_eq!(command.class(), ConversationEventClass::Command);
    assert_eq!(completed.class(), ConversationEventClass::Fact);
}

#[test]
fn conversation_event_envelope_preserves_its_classification_and_payload() {
    let envelope = ConversationEventEnvelope::new(
        ConversationEventClass::Command,
        "test".to_owned(),
        "1".to_owned(),
        "invocation_requested".to_owned(),
        1,
        "An invocation was requested.".to_owned(),
        json!({ "invocation_id": "invocation_1" }),
    )
    .expect("the conversation event envelope should be valid");
    let serialized = serde_json::to_value(&envelope).expect("the envelope should serialize");
    let restored: ConversationEventEnvelope =
        serde_json::from_value(serialized).expect("the envelope should deserialize");

    assert_eq!(restored.class(), ConversationEventClass::Command);
    assert_eq!(restored.namespace(), "test");
    assert_eq!(restored.namespace_version(), "1");
    assert_eq!(restored.event_type(), "invocation_requested");
    assert_eq!(restored.event_schema_version(), 1);
    assert_eq!(restored.description(), "An invocation was requested.");
    assert_eq!(restored.payload()["invocation_id"], "invocation_1");

    let fact = ConversationEventEnvelope::new(
        ConversationEventClass::Fact,
        "test".to_owned(),
        "1".to_owned(),
        "invocation_finished".to_owned(),
        1,
        "An invocation finished.".to_owned(),
        json!({ "successful": true }),
    )
    .expect("the conversation fact envelope should be valid");
    assert_eq!(fact.class(), ConversationEventClass::Fact);
}

#[test]
fn assistant_response_rejects_an_empty_message() {
    assert_eq!(
        AssistantResponse::new(String::new()),
        Err(InvalidAssistantResponse::EmptyMessage)
    );
}

#[test]
fn model_communication_validates_messages_and_subtypes() {
    assert_eq!(
        ModelCommunication::new(
            String::new(),
            ModelEventImportance::Detailed,
            "reasoning".to_owned(),
        ),
        Err(InvalidModelCommunication::EmptyMessage)
    );
    assert_eq!(
        ModelCommunication::new(
            "message".to_owned(),
            ModelEventImportance::Detailed,
            " ".to_owned(),
        ),
        Err(InvalidModelCommunication::EmptySubtype)
    );
}
