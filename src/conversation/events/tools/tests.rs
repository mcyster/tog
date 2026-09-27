use schemars::json_schema;

use super::{InvalidToolDefinition, Tool, ToolAvailability, ToolDefinition, ToolName, Tools};

fn tool_definition(name: &str) -> ToolDefinition {
    ToolDefinition::try_new(
        ToolName::try_new(name.to_owned()).expect("the tool name should be valid"),
        "Run a command.".to_owned(),
        json_schema!({ "type": "object" }),
        json_schema!({ "type": "object" }),
    )
    .expect("the tool definition should be valid")
}

#[test]
fn tool_names_reject_blank_values() {
    assert_eq!(
        ToolName::try_new("   ".to_owned()),
        Err(InvalidToolDefinition::EmptyToolName)
    );
    assert!(serde_json::from_str::<ToolName>("\"  \"").is_err());
}

#[test]
fn tool_definitions_reject_a_blank_description() {
    assert_eq!(
        ToolDefinition::try_new(
            ToolName::try_new("shell".to_owned()).expect("the tool name should be valid"),
            "  ".to_owned(),
            json_schema!({ "type": "object" }),
            json_schema!({ "type": "object" }),
        ),
        Err(InvalidToolDefinition::EmptyDescription)
    );
}

#[test]
fn tools_round_trip_their_definitions_and_availability() {
    let tools = Tools::new(vec![
        Tool::new(tool_definition("shell"), ToolAvailability::Immediate),
        Tool::new(tool_definition("search"), ToolAvailability::Discoverable),
    ]);

    let restored: Tools =
        serde_json::from_value(serde_json::to_value(&tools).expect("the tools should serialize"))
            .expect("the tools should deserialize");
    assert_eq!(restored, tools);
    assert_eq!(restored.tools().len(), 2);
    assert_eq!(
        restored.tools()[0].availability(),
        ToolAvailability::Immediate
    );
    assert_eq!(
        restored.tools()[1].availability(),
        ToolAvailability::Discoverable
    );
    assert_eq!(restored.tools()[0].definition().name().as_str(), "shell");
    assert!(
        restored.tools()[1]
            .definition()
            .parameters()
            .as_value()
            .is_object()
    );
}

#[test]
fn availability_serializes_its_policy_names() {
    assert_eq!(
        serde_json::to_value(ToolAvailability::Immediate).expect("the value should serialize"),
        serde_json::json!("immediate")
    );
    assert_eq!(
        serde_json::to_value(ToolAvailability::Discoverable).expect("the value should serialize"),
        serde_json::json!("discoverable")
    );
}

#[test]
fn empty_tools_are_valid_and_clear_available_tools() {
    let tools = Tools::new(Vec::new());
    assert!(tools.tools().is_empty());
    tools
        .ensure_valid()
        .expect("an empty tools declaration should be valid");
}

#[test]
fn tools_reject_an_invalid_deserialized_definition() {
    let tools: Tools = serde_json::from_value(serde_json::json!({
        "tools": [{
            "definition": {
                "name": "shell",
                "description": "   ",
                "parameters": { "type": "object" },
                "result": { "type": "object" }
            },
            "availability": "immediate"
        }]
    }))
    .expect("the tools should deserialize");

    assert_eq!(
        tools.ensure_valid(),
        Err(super::InvalidTools::Definition(
            InvalidToolDefinition::EmptyDescription
        ))
    );
}
