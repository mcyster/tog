use schemars::json_schema;

use super::{
    InvalidToolDefinition, ToolAvailability, ToolDefinition, ToolName, Toolset, ToolsetEntry,
};

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
fn a_toolset_round_trips_its_entries_and_availability() {
    let toolset = Toolset::new(vec![
        ToolsetEntry::new(tool_definition("shell"), ToolAvailability::Immediate),
        ToolsetEntry::new(tool_definition("search"), ToolAvailability::Discoverable),
    ])
    .expect("the toolset should be valid");

    let restored: Toolset = serde_json::from_value(
        serde_json::to_value(&toolset).expect("the toolset should serialize"),
    )
    .expect("the toolset should deserialize");
    assert_eq!(restored, toolset);
    assert_eq!(restored.entries().len(), 2);
    assert_eq!(
        restored.entries()[0].availability(),
        ToolAvailability::Immediate
    );
    assert_eq!(
        restored.entries()[1].availability(),
        ToolAvailability::Discoverable
    );
    assert_eq!(restored.entries()[0].definition().name().as_str(), "shell");
    assert!(
        restored.entries()[1]
            .definition()
            .parameters()
            .as_value()
            .is_object()
    );
}

#[test]
fn an_empty_toolset_is_valid_and_clears_tools() {
    let toolset = Toolset::new(Vec::new()).expect("an empty toolset should be valid");
    assert!(toolset.entries().is_empty());
}

#[test]
fn an_immediate_toolset_maps_every_definition_to_immediate_availability() {
    let toolset = Toolset::immediate(vec![tool_definition("shell"), tool_definition("search")])
        .expect("the toolset should be valid");
    assert!(
        toolset
            .entries()
            .iter()
            .all(|entry| entry.availability() == ToolAvailability::Immediate)
    );
}

#[test]
fn a_toolset_rejects_an_invalid_deserialized_definition() {
    let toolset: Toolset = serde_json::from_value(serde_json::json!({
        "entries": [{
            "definition": {
                "name": "shell",
                "description": "   ",
                "parameters": { "type": "object" },
                "result": { "type": "object" }
            },
            "availability": "immediate"
        }]
    }))
    .expect("the toolset should deserialize");

    assert_eq!(
        toolset.ensure_valid(),
        Err(super::InvalidToolset::Definition(
            InvalidToolDefinition::EmptyDescription
        ))
    );
}
