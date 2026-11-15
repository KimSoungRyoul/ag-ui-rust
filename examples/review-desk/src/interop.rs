//! Export real SDK operations for the separately installed official web core.

use ag_ui::server::RunContext;
use ag_ui::{Event, RunAgentInput};
use ag_ui_a2ui::adapters::agui::{Painted, SurfacePainter, unwrap_operations_envelope};
use ag_ui_a2ui::constants::OFFICIAL_BASIC_CATALOG_ID;
use ag_ui_a2ui::{A2uiAuthor, A2uiVersion, AgentMessage, Component};
use serde_json::{Value, json};

use crate::client::AppResult;

struct Step {
    name: &'static str,
    messages: Vec<AgentMessage>,
}

/// What a run sent for one paint: the tool result and the activities after it.
fn sent(events: Vec<Event>) -> AppResult<Painted> {
    let mut envelope = None;
    let mut activities = Vec::new();
    for event in events {
        match event {
            Event::ToolCallResult(result) => envelope = Some(result.content.to_text()),
            Event::ActivitySnapshot(_) => activities.push(event),
            _ => {}
        }
    }
    Ok(Painted {
        envelope: envelope.ok_or("a paint without a tool result")?,
        activities,
    })
}

/// Serializes what the AG-UI adapter sends for authored and typed low-level
/// A2UI updates: each batch as its tool result carries it, and the surface
/// activity as it stands after the batch. No field is changed by the
/// JavaScript consumer before processing.
pub async fn fixture() -> AppResult<Value> {
    let version = A2uiVersion::V0_9_1;
    let id = "official-interop";
    let author = A2uiAuthor::basic(version)?;
    let initial = vec![
        AgentMessage::create_surface(id, OFFICIAL_BASIC_CATALOG_ID).with_version(version),
        AgentMessage::update_components(
            id,
            vec![
                Component::new("root", "Column").with("children", json!(["title"])),
                Component::new("title", "Text").with("text", json!({"path": "/title"})),
            ],
        )
        .with_version(version),
        AgentMessage::update_data_model(id, "/", json!({"title": "Original review", "memo": null}))
            .with_version(version),
    ];
    let response = serde_json::to_string(&initial)?;
    let created = author
        .create(id, "Create the review card")
        .generate(|_, _| {
            let response = response.clone();
            async move { Ok(response) }
        })
        .await?;
    let change = AgentMessage::update_data_model(id, "/title", json!("Reviewed title"))
        .with_version(version);
    let response = serde_json::to_string(&[change])?;
    let edited = author
        .edit(&created, "Update only the title")
        .generate(|_, _| {
            let response = response.clone();
            async move { Ok(response) }
        })
        .await?;

    let mut steps = vec![
        Step {
            name: "create",
            messages: created.operations().to_vec(),
        },
        Step {
            name: "edit",
            messages: edited.operations().to_vec(),
        },
    ];
    steps.push(Step {
        name: "explicit-null",
        messages: vec![
            AgentMessage::update_data_model(id, "/memo", Value::Null).with_version(version),
            AgentMessage::update_data_model(id, "/discard", json!("temporary"))
                .with_version(version),
            AgentMessage::update_data_model(id, "/items", json!([1, null, 3]))
                .with_version(version),
        ],
    });
    steps.push(Step {
        name: "upsert-missing-array",
        messages: vec![
            AgentMessage::update_data_model(id, "/list/0", json!("first")).with_version(version),
        ],
    });
    steps.push(Step {
        name: "upsert-null-parent",
        messages: vec![
            AgentMessage::update_data_model(id, "/nullable", Value::Null).with_version(version),
            AgentMessage::update_data_model(id, "/nullable/name", json!("nested"))
                .with_version(version),
            AgentMessage::update_data_model(id, "/nullableArray", Value::Null)
                .with_version(version),
            AgentMessage::update_data_model(id, "/nullableArray/0", json!("first"))
                .with_version(version),
        ],
    });
    steps.push(Step {
        name: "upsert-sparse-array",
        messages: vec![
            AgentMessage::update_data_model(id, "/list/3", json!("fourth")).with_version(version),
            AgentMessage::update_data_model(id, "/sparse/2", json!("third")).with_version(version),
            AgentMessage::remove_data_model_value(id, "/list/5").with_version(version),
        ],
    });
    steps.push(Step {
        name: "upsert-nested-array",
        messages: vec![
            AgentMessage::update_data_model(id, "/matrix/0/1", json!("inner"))
                .with_version(version),
        ],
    });
    steps.push(Step {
        name: "remove-object-key",
        messages: vec![AgentMessage::remove_data_model_value(id, "/discard").with_version(version)],
    });
    steps.push(Step {
        name: "remove-array-slot",
        messages: vec![AgentMessage::remove_data_model_value(id, "/items/1").with_version(version)],
    });
    steps.push(Step {
        name: "replace-component",
        messages: vec![
            AgentMessage::update_components(
                id,
                vec![Component::new("title", "Text").with("text", json!("Replacement text"))],
            )
            .with_version(version),
        ],
    });
    steps.push(Step {
        name: "upsert-null-root",
        messages: vec![
            AgentMessage::update_data_model(id, "/", Value::Null).with_version(version),
            AgentMessage::update_data_model(id, "/name", json!("root restored"))
                .with_version(version),
        ],
    });
    steps.push(Step {
        name: "delete",
        messages: vec![AgentMessage::delete_surface(id).with_version(version)],
    });
    steps.push(Step {
        name: "recreate",
        messages: vec![
            AgentMessage::create_surface(id, OFFICIAL_BASIC_CATALOG_ID).with_version(version),
            AgentMessage::update_components(
                id,
                vec![Component::new("root", "Text").with("text", json!("Recreated surface"))],
            )
            .with_version(version),
            AgentMessage::update_data_model(id, "/", json!({"title":"Fresh review"}))
                .with_version(version),
        ],
    });

    // Every step is painted by the adapter, in one run. The authored two go
    // through a `RunContext`, so what is exported is what `SurfacePainter::paint`
    // put in TOOL_CALL_RESULT and ACTIVITY_SNAPSHOT; the rest are settled
    // without one, which is the same code less the send. The painter's store is
    // the SDK's public state machine. Undefined is preserved locally; no
    // snapshot value is sent as wire data.
    let (mut ctx, mut receiver) = RunContext::<()>::new(RunAgentInput::new("interop", "export"))?;
    let mut painter = SurfacePainter::new();
    let mut exported_steps = Vec::new();
    for (index, step) in steps.iter().enumerate() {
        let painted = if index < 2 {
            painter.paint(&mut ctx, &step.messages)?;
            sent(receiver.drain())?
        } else {
            painter.settle(&format!("call-{index}"), &step.messages)?
        };
        let batch = unwrap_operations_envelope(&serde_json::from_str(&painted.envelope)?)?;
        let [Event::ActivitySnapshot(activity)] = painted.activities.as_slice() else {
            return Err("one surface is painted as one activity".into());
        };
        let data = painter
            .surfaces()
            .get(id)
            .filter(|surface| !surface.deleted)
            .map(|surface| &surface.data_model);
        exported_steps.push(json!({
            "name": step.name,
            "messages": batch,
            "activity": {
                "messageId": activity.message_id,
                "activityType": activity.activity_type,
                "replace": activity.replace,
                "content": activity.content,
            },
            "sdkData": data,
        }));
    }
    let final_data = painter
        .surfaces()
        .get(id)
        .ok_or("recreated surface missing")?
        .data_model
        .to_json()?;
    Ok(
        json!({"version": version.as_str(), "catalogId": OFFICIAL_BASIC_CATALOG_ID,
        "surfaceId": id, "steps": exported_steps, "finalSdkData": final_data}),
    )
}
