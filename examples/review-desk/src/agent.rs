//! Domain behavior and event emission; no model credentials are required.

use ag_ui::server::{Agent, Error, Result, RunContext};
use ag_ui::{Event, Interrupt, ResumeStatus, RunOutcome};
use ag_ui_a2ui::agui::A2uiRunContextExt;
use ag_ui_a2ui::{A2uiAuthor, A2uiVersion};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicU32, Ordering};

/// Shared state that travels in the AG-UI request and state events.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReviewState {
    /// The item being reviewed.
    pub title: String,
    /// Findings owned by this application.
    pub checks: Vec<String>,
    /// The two human decisions, after a completed resume request.
    pub decisions: Vec<String>,
    /// A visible counter for thread isolation and snapshot restoration.
    pub revision: u32,
}

/// A deterministic local assistant, with deliberately fallible review steps.
#[derive(Debug)]
pub struct ReviewDesk;

impl Agent for ReviewDesk {
    type State = ReviewState;

    async fn run(&self, ctx: &mut RunContext<ReviewState>) -> Result<RunOutcome> {
        let command = ctx.last_user_text().unwrap_or_default();
        match command.as_str() {
            "approve" => decisions(ctx),
            "surface" => surface(ctx).await,
            "slow" => {
                ctx.say("Waiting for the local review worker. Use abort to stop receiving.")?;
                ctx.emit(Event::custom("progress", json!({"percent": 10})))?;
                ctx.until_cancelled(std::future::pending::<()>()).await;
                Err(Error::Cancelled)
            }
            "status" => {
                ctx.publish_state()?;
                ctx.say(format!(
                    "{}: {} checks, revision {}.",
                    ctx.state().title,
                    ctx.state().checks.len(),
                    ctx.state().revision,
                ))?;
                Ok(RunOutcome::Success)
            }
            _ => review(ctx, command.strip_prefix("review ").unwrap_or(&command)).await,
        }
    }
}

async fn review(ctx: &mut RunContext<ReviewState>, title: &str) -> Result<RunOutcome> {
    ctx.update_state(|state| {
        state.title = title.to_owned();
        state.checks.clear();
        state.decisions.clear();
        state.revision += 1;
    })?;
    ctx.emit(Event::custom("progress", json!({"percent": 0})))?;

    // The application performs the work. This handle only attributes events.
    {
        let mut events = ctx.subagent_events("checklist")?;
        events.say("Checking ownership and reproducible validation.")?;
        tokio::task::yield_now().await;
        events.update_state(|state| {
            state
                .checks
                .push("Owner and validation steps recorded".into());
        })?;
        events.finish_with(json!({"checks": 1}))?;
    }
    ctx.emit(Event::custom("progress", json!({"percent": 50})))?;

    {
        let mut events = ctx.subagent_events("risk-check")?;
        if title.contains("broken") {
            events.say("The local dependency fixture is unavailable.")?;
            events.fail("Dependency fixture unavailable; manual review required")?;
            ctx.update_state(|state| {
                state
                    .checks
                    .push("Manual dependency review required".into());
            })?;
        } else {
            events.say("No unresolved dependencies in this local fixture.")?;
            events.update_state(|state| state.checks.push("Dependencies checked".into()))?;
            events.finish_with(json!({"dependencies": "checked"}))?;
        }
    }

    let mut message = ctx.assistant_message()?;
    for word in ["Review ", "prepared. ", "Two ", "decisions ", "remain."] {
        message.delta(word)?;
        tokio::task::yield_now().await;
    }
    message.end()?;
    ctx.emit(Event::custom("progress", json!({"percent": 100})))?;
    Ok(RunOutcome::Success)
}

fn decisions(ctx: &mut RunContext<ReviewState>) -> Result<RunOutcome> {
    const DECISIONS: [(&str, &str); 2] = [
        ("accept-review", "Accept the review findings?"),
        ("publish-review", "Mark the local review ready to publish?"),
    ];
    let answers: Option<Vec<_>> = DECISIONS
        .iter()
        .map(|(id, _)| ctx.resume_for(id).map(|entry| (*id, entry.status)))
        .collect();

    let Some(answers) = answers else {
        ctx.say("Both decisions are required before continuing.")?;
        return Ok(RunOutcome::interrupt(
            DECISIONS
                .iter()
                .map(|(id, question)| Interrupt {
                    id: (*id).into(),
                    reason: "tool_approval".into(),
                    message: Some((*question).into()),
                    response_schema: Some(
                        json!({
                            "type": "object", "properties": {"approved": {"const": true}},
                            "required": ["approved"], "additionalProperties": false
                        })
                        .as_object()
                        .expect("literal object")
                        .clone(),
                    ),
                    ..Interrupt::default()
                })
                .collect::<Vec<_>>(),
        ));
    };

    // This domain rule belongs to the server. An SDK responseSchema hint
    // does not authorize an operation or replace application validation.
    for (id, status) in &answers {
        if *status == ResumeStatus::Resolved
            && ctx.resume_for(id).and_then(|entry| entry.payload.as_ref())
                != Some(&json!({"approved": true}))
        {
            return Err(Error::agent("approval payload must be {\"approved\":true}"));
        }
    }

    ctx.update_state(|state| {
        state.decisions = answers
            .iter()
            .map(|(id, status)| {
                format!(
                    "{id}: {}",
                    if *status == ResumeStatus::Resolved {
                        "approved"
                    } else {
                        "declined"
                    }
                )
            })
            .collect();
        state.revision += 1;
    })?;
    ctx.say(ctx.state().decisions.join("; "))?;
    Ok(RunOutcome::Success)
}

async fn surface(ctx: &mut RunContext<ReviewState>) -> Result<RunOutcome> {
    let author = A2uiAuthor::basic(A2uiVersion::V0_9_1).map_err(Error::agent)?;
    // A new visible card per invocation avoids recreating an active surface.
    let id = format!("review-{}", ctx.run_id());
    let title = ctx.state().title.clone();
    let response = create_response(&id, &title);
    let attempts = AtomicU32::new(0);
    let card = author
        .create(
            &id,
            "Create a review card with a bound title and a nullable memo",
        )
        .generate(|_prompt, attempt| {
            attempts.store(attempt, Ordering::Relaxed);
            let response = response.clone();
            async move {
                tokio::task::yield_now().await;
                // Exercise the SDK's correction loop, not an application retry.
                Ok(if attempt == 1 {
                    "not valid JSON".into()
                } else {
                    response
                })
            }
        })
        .await
        .map_err(Error::agent)?;
    ctx.emit(Event::custom(
        "a2ui-generation",
        json!({"attempts": attempts.load(Ordering::Relaxed)}),
    ))?;
    ctx.send_a2ui(&card)?;

    let patch = json!([{
        "version": "v0.9.1",
        "updateDataModel": {"surfaceId": id, "path": "/title", "value": format!("{title} · reviewed")}
    }])
    .to_string();
    let edited = author
        .edit(
            &card,
            "Append the reviewed label to the title; preserve memo",
        )
        .generate(|_prompt, _attempt| {
            let patch = patch.clone();
            async move { Ok(patch) }
        })
        .await
        .map_err(Error::agent)?;
    ctx.send_a2ui(&edited)?;
    ctx.say("Created the review card, then edited its title. Memo remains null.")?;
    Ok(RunOutcome::Success)
}

/// A stand-in for a model response: real A2UI wire JSON, checked by the SDK.
pub fn create_response(id: &str, title: &str) -> String {
    let operations: Value = json!([
        {"version": "v0.9.1", "createSurface": {
            "surfaceId": id,
            "catalogId": "https://a2ui.org/specification/v0_9/catalogs/basic/catalog.json"
        }},
        {"version": "v0.9.1", "updateComponents": {
            "surfaceId": id,
            "components": [
                {"id": "root", "component": "Column", "children": ["title"]},
                {"id": "title", "component": "Text", "text": {"path": "/title"}}
            ]
        }},
        {"version": "v0.9.1", "updateDataModel": {
            "surfaceId": id, "path": "/", "value": {"title": title, "memo": null}
        }}
    ]);
    operations.to_string()
}
