//! Live smoke test: a real streaming model, mapped onto AG-UI, over real HTTP.
//!
//! ```text
//! export GEMINI_API_KEY=…            # or AG_UI_LLM_API_KEY
//! cargo test -p ag-ui-e2e --test live_llm -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! Or against a model on your own machine, where nothing is rate limited and no
//! key is needed at all:
//!
//! ```text
//! ollama serve && ollama pull qwen3:4b
//! export AG_UI_LLM_BASE_URL=http://localhost:11434/v1
//! export AG_UI_LLM_MODEL=qwen3:4b
//! cargo test -p ag-ui-e2e --test live_llm -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! Or with a Qwen Cloud subscription, whose OpenAI-compatible mode is
//! recognised by name — no `AG_UI_LLM_*` needed:
//!
//! ```text
//! export QWEN_API_KEY=…
//! export QWEN_BASE_URL=https://dashscope-intl.aliyuncs.com/compatible-mode/v1
//! export QWEN_MODEL=qwen3.8-flash    # qwen-plus when unset; `$QWEN_BASE_URL/models` lists what yours serves
//! cargo test -p ag-ui-e2e --test live_llm -- --ignored --test-threads=1 --nocapture
//! ```
//!
//! `--nocapture` is worth typing: a run that skips, and the model's actual
//! reply, are printed rather than asserted, and the harness swallows the output
//! of a test that passed.
//!
//! Every test here is `#[ignore]`, so `cargo test` and CI never touch the
//! network. Without a key the tests skip rather than fail — a contributor who
//! has no key should still see a green run.
//!
//! **Expect a hosted free tier to run out.** Gemini's allows only about **20
//! requests per model per day**, not just the 10 per minute the docs talk about,
//! and both limits are shared with anything else using the same key. When the
//! day's quota is gone this file falls back to sibling models, and when none of
//! them can answer it **skips and says so** rather than failing.
//!
//! # What it proves that the deterministic tier cannot
//!
//! Not the mapping — `ag_ui_e2e::llm`'s own unit tests cover that, driven from
//! recorded frames, and they are what actually protect the parsing and the
//! argument accumulation. This file proves the wire is reachable: that the
//! protocol plumbing survives a model streaming on its own schedule, and that
//! the SDK genuinely needs no LLM crate to talk to one. [`LlmAgent`] is
//! `reqwest` and `serde` and nothing else. See `docs/QA.md` for the mapping and
//! `docs/DESIGN.md` for why the second claim matters.
//!
//! # A failure here must mean the mapping is wrong
//!
//! This suite talks to someone else's capacity-constrained service, so most of
//! the ways it can not-work say nothing at all about this SDK. A `503 high
//! demand` reported as a test failure costs somebody an hour looking for a bug
//! that is not there, so the harness sorts the outcomes:
//!
//! | What came back | What happens |
//! | --- | --- |
//! | A stream | Asserted on, loudly. This is the point of the file. |
//! | `429` naming a per-minute quota | Wait for `RetryInfo.retryDelay`, ask again |
//! | `500`, `502`, `503`, `504` | Transient by definition — back off and retry, then try the next model |
//! | `429` naming a per-day quota | Cannot be waited out; move to the next model |
//! | `404` | That model does not exist here; move to the next |
//! | Nothing answered on the socket | **Skip** — the endpoint is not up |
//! | No model left | **Skip**, naming which model failed how |
//! | Anything else | Fail loudly — a `400` or an agent error is ours |
//!
//! Quota is isolated per model, which is what makes falling back work at all.
//!
//! # The rate limits shape this file
//!
//! Both limits were measured, not documented, and neither is reported in a
//! response header — the numbers come out of `429` bodies. So:
//!
//! - The whole file spends **four** requests when everything works: one for the
//!   text run, two for the tool run (the call, then the answer to its result),
//!   and one for the delegated run.
//! - Runs are serialized by [`LIVE`] as well as by `--test-threads=1`, because
//!   two parallel tests trip the per-minute limit immediately.
//! - Waiting happens only where the provider asked for it, and the whole file
//!   will not sleep for longer than [`WAIT_BUDGET`] all told.

use std::time::Duration;

use ag_ui::axum::RouterExt;
use ag_ui::client::{Applier, HttpAgent, RunParams, verify_all};
use ag_ui::server::{Agent, Result, RunContext};
use ag_ui::{Event, EventType, RunOutcome};
use ag_ui_e2e::llm::{DEFAULT_BASE_URL, LlmAgent, WEATHER_TOOL};
use axum::Router;
use futures_util::StreamExt as _;
use serde_json::Value;

/// Serializes live runs even when the harness was not told to.
///
/// `--test-threads=1` is documented above, but a forgotten flag should cost a
/// slow test run rather than a burst of `429`s. Each `#[tokio::test]` builds its
/// own runtime, and this is held across the whole run, so the lock has to be the
/// async one.
static LIVE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Models to fall back to when the configured one will not answer, in
/// preference order.
///
/// Only used against the default endpoint, and only because quota there is
/// isolated per model — verified: with flash-lite exhausted, both of these still
/// answered. A 3.x model is safe to fall back to now that the harness speaks the
/// OpenAI-compatible format; on the native dialect it was a `400`, because 3.x
/// requires a `thoughtSignature` echoed back and 2.5 sends none.
const FALLBACKS: [&str; 2] = ["gemini-2.5-flash", "gemini-3.1-flash-lite"];

/// How many times one model is asked before moving to the next.
const MAX_ATTEMPTS: u32 = 3;

/// The first back-off for a transient failure that carried no `RetryInfo`.
/// Doubles per attempt.
const BACKOFF: Duration = Duration::from_secs(2);

/// What to wait for a rate limit that carried no `RetryInfo`.
const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(30);

/// The provider asking for longer than this is asking for a different test run.
const MAX_RETRY: Duration = Duration::from_secs(90);

/// How long the whole file may spend asleep, across every retry of every model.
/// Past this, an unanswered run is a skip rather than a longer wait.
const WAIT_BUDGET: Duration = Duration::from_secs(150);

/// A live run, streamed to the client, produces a well-ordered AG-UI stream
/// with real text in it.
#[tokio::test]
#[ignore = "spends live model quota; run with --ignored"]
async fn text_streams_as_a_well_ordered_run() {
    let Some(live) = live().await else { return };
    let _serialized = LIVE.lock().await;

    let Some(events) = run(
        &live,
        RunParams::new("live-thread", "live-text").user("m1", "Reply with the single word: pong"),
    )
    .await
    else {
        return;
    };

    // The SDK's own client-side verifier, on a stream the SDK did not script.
    verify_all(&events).unwrap_or_else(|error| panic!("{error}\n{}", summary(&events)));

    let types = types(&events);
    assert_eq!(types.first(), Some(&EventType::RunStarted), "{types:?}");
    assert_eq!(types.last(), Some(&EventType::RunFinished), "{types:?}");
    assert!(
        types.contains(&EventType::TextMessageStart) && types.contains(&EventType::TextMessageEnd),
        "the reply should be bracketed: {types:?}"
    );

    let deltas = deltas(&events);
    assert!(!deltas.is_empty(), "no text arrived: {}", summary(&events));
    // The final frame carries `finish_reason` and often no content at all. An
    // empty delta is not an update, and must not be forwarded as one.
    assert!(
        deltas.iter().all(|delta| !delta.is_empty()),
        "an empty TEXT_MESSAGE_CONTENT went out: {deltas:?}"
    );

    // The completion id is stable for the whole stream, so one turn is one
    // message.
    let ids: Vec<&str> = events
        .iter()
        .filter_map(|event| match event {
            Event::TextMessageContent(payload) => Some(payload.message_id.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        ids.windows(2).all(|pair| pair[0] == pair[1]),
        "one turn should stream under one message id: {ids:?}"
    );

    let text = deltas.concat();
    assert!(!text.trim().is_empty(), "the reply was blank");
    eprintln!("live reply: {text:?} in {} events", events.len());
}

/// A prompt the model answers with a tool call round-trips: the call is
/// streamed as AG-UI events, executed here, reported back, and the model's
/// final answer uses the result.
#[tokio::test]
#[ignore = "spends live model quota; run with --ignored"]
async fn a_tool_call_round_trips_through_the_protocol() {
    let Some(live) = live().await else { return };
    let _serialized = LIVE.lock().await;

    let Some(events) = run(
        &live,
        RunParams::new("live-thread", "live-tool")
            .user("m1", "What is the weather in Seoul right now?"),
    )
    .await
    else {
        return;
    };

    verify_all(&events).unwrap_or_else(|error| panic!("{error}\n{}", summary(&events)));

    let types = types(&events);
    for expected in [
        EventType::ToolCallStart,
        EventType::ToolCallArgs,
        EventType::ToolCallEnd,
        EventType::ToolCallResult,
    ] {
        assert!(
            types.contains(&expected),
            "{expected} is missing: {}",
            summary(&events)
        );
    }

    // Start, then args, then end, then the result — in that order.
    let position = |wanted: EventType| types.iter().position(|kind| *kind == wanted);
    let order = [
        position(EventType::ToolCallStart),
        position(EventType::ToolCallArgs),
        position(EventType::ToolCallEnd),
        position(EventType::ToolCallResult),
    ];
    assert!(
        order.windows(2).all(|pair| pair[0] < pair[1]),
        "tool events are out of order: {types:?}"
    );

    let (id, name) = events
        .iter()
        .find_map(|event| match event {
            Event::ToolCallStart(payload) => {
                Some((payload.tool_call_id.clone(), payload.tool_call_name.clone()))
            }
            _ => None,
        })
        .expect("the call was asserted above");
    assert_eq!(name, WEATHER_TOOL);
    // On this wire format the id is the server's own, carried through all four
    // events rather than invented here.
    assert!(!id.is_empty(), "the tool call had no id");

    // The arguments arrive as partial JSON spread across frames, so what the
    // client receives has to concatenate back into something parseable.
    let arguments: String = events
        .iter()
        .filter_map(|event| match event {
            Event::ToolCallArgs(payload) if payload.tool_call_id == id => {
                Some(payload.delta.as_str())
            }
            _ => None,
        })
        .collect();
    let arguments: Value = serde_json::from_str(&arguments)
        .unwrap_or_else(|error| panic!("tool arguments did not parse: {arguments:?}: {error}"));
    let city = arguments["city"].as_str().unwrap_or_default();
    assert!(
        city.to_lowercase().contains("seoul"),
        "the model was asked about Seoul: {arguments}"
    );

    let result = events
        .iter()
        .find_map(|event| match event {
            Event::ToolCallResult(payload) if payload.tool_call_id == id => {
                Some(payload.content.clone())
            }
            _ => None,
        })
        .expect("the result was asserted above");
    assert!(result.contains("21"), "the tool's own reading: {result}");

    // The second request fed the tool message back, so the final answer should
    // be about what the tool actually returned.
    let answer = deltas(&events).concat();
    assert!(
        !answer.trim().is_empty(),
        "no final answer: {}",
        summary(&events)
    );
    let lowered = answer.to_lowercase();
    assert!(
        lowered.contains("21") || lowered.contains("clear"),
        "the answer ignored the tool result: {answer:?}"
    );
    assert_eq!(types.last(), Some(&EventType::RunFinished), "{types:?}");
    eprintln!("live answer: {answer:?}");
}

/// One endpoint per model to try, all on one server.
struct Live {
    endpoints: Vec<(String, String)>,
}

/// Mounts the agents on a real router and binds a real port, or skips.
///
/// The run goes over loopback HTTP through `ag-ui-client` rather than straight
/// into the agent: the point is to exercise the whole stack, SSE encoding and
/// decoding included.
///
/// Every model is mounted up front because mounting costs nothing — an
/// [`LlmAgent`] makes no request until it is run — and because switching
/// afterwards would mean standing up a second server mid-test.
async fn live() -> Option<Live> {
    live_with(|agent| agent).await
}

/// [`live`], with each model's agent wrapped — in a supervisor, say.
async fn live_with<A: Agent + 'static>(wrap: impl Fn(LlmAgent) -> A) -> Option<Live> {
    let agent = match LlmAgent::from_env() {
        Ok(agent) => agent,
        Err(error) => {
            // Names the variable, never a value.
            eprintln!("SKIPPED: {error}");
            return None;
        }
    };

    // Fallbacks are a Gemini free-tier workaround. Anywhere else — a local
    // server most of all — the configured model is the only one that exists.
    let mut models = vec![agent.model_name().to_owned()];
    if agent.base_url() == DEFAULT_BASE_URL {
        models.extend(
            FALLBACKS
                .iter()
                .filter(|model| **model != agent.model_name())
                .map(|model| (*model).to_owned()),
        );
    }
    eprintln!("live endpoint: {} | models: {models:?}", agent.base_url());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port");
    let address = listener.local_addr().expect("a bound address");

    let mut app = Router::new();
    let mut endpoints = Vec::with_capacity(models.len());
    for (index, model) in models.into_iter().enumerate() {
        let path = format!("/agent-{index}");
        let agent = LlmAgent::from_env()
            .expect("the configuration was read a moment ago")
            .model(model.clone());
        app = app.route_agui(&path, wrap(agent));
        endpoints.push((model, format!("http://{address}{path}")));
    }

    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    Some(Live { endpoints })
}

/// Runs once, working around whatever the provider is doing today.
///
/// `None` means no model could be reached, which is not a result about this
/// SDK — the caller returns rather than asserting on a stream that never
/// happened.
async fn run(live: &Live, params: RunParams) -> Option<Vec<Event>> {
    let mut budget = WAIT_BUDGET;
    let mut refused: Vec<String> = Vec::new();

    for (model, endpoint) in &live.endpoints {
        for attempt in 1..=MAX_ATTEMPTS {
            let events = collect(endpoint, params.clone()).await;
            let Some(message) = run_error(&events) else {
                return Some(events);
            };

            match classify(message, attempt) {
                // The one outcome worth failing on: nothing upstream refused,
                // so the agent or the SDK produced this.
                Upstream::Ours => panic!("the run failed: {message}"),

                Upstream::Unavailable(reason) => {
                    eprintln!("{model}: {reason}");
                    refused.push(format!("{model}: {reason}"));
                    break;
                }

                Upstream::Transient { delay, reason } if attempt < MAX_ATTEMPTS => {
                    if delay > budget {
                        let reason = format!("{reason}, and waiting {delay:?} is over budget");
                        eprintln!("{model}: {reason}");
                        refused.push(format!("{model}: {reason}"));
                        break;
                    }
                    eprintln!("{model}: {reason}; waiting {delay:?} and asking again");
                    tokio::time::sleep(delay).await;
                    budget -= delay;
                }

                Upstream::Transient { reason, .. } => {
                    let reason = format!("{reason}, still there after {MAX_ATTEMPTS} attempts");
                    eprintln!("{model}: {reason}");
                    refused.push(format!("{model}: {reason}"));
                }
            }
        }
    }

    eprintln!("SKIPPED: no model answered, so the AG-UI mapping was never exercised.");
    eprintln!("This is upstream quota or capacity, not an SDK failure:");
    for reason in &refused {
        eprintln!("  - {reason}");
    }
    None
}

/// One run's events, exactly as the agent sent them.
async fn collect(url: &str, params: RunParams) -> Vec<Event> {
    let agent = HttpAgent::http(url).expect("the URL was just built from an address");
    agent
        .run(params)
        .map(|event| event.expect("the transport should not break over loopback"))
        .collect()
        .await
}

/// Why a run produced no stream.
enum Upstream {
    /// The provider is busy or throttling. Wait, then ask it again.
    Transient { delay: Duration, reason: String },
    /// This model will not answer today, however long anyone waits.
    Unavailable(String),
    /// Nothing upstream refused anything — this failure is the SDK's or the
    /// agent's, and it is what the suite exists to catch.
    Ours,
}

/// Sorts a `RUN_ERROR` into "the provider said no" and "we are wrong".
///
/// The provider's status and error body travel in the `RUN_ERROR` message,
/// which is where they have to be read from: there are no `X-RateLimit-*`
/// headers, and by the time the client sees the failure the HTTP exchange with
/// the model is long over. [`LlmAgent`] formats them as
/// `the model returned HTTP {status}: {body}`.
fn classify(message: &str, attempt: u32) -> Upstream {
    let Some(status) = status_of(message) else {
        // Nothing answered on the socket at all — a local server that is not
        // running, or DNS. That is a misconfiguration, not a mapping bug, and
        // reporting it as a failed assertion helps nobody.
        if message.contains("error sending request") || message.contains("connect") {
            return Upstream::Unavailable("the endpoint could not be reached".to_owned());
        }
        return Upstream::Ours;
    };

    match status {
        // The body names the quota it enforced, for example
        // `GenerateRequestsPerDayPerProjectPerModel-FreeTier`. A day does not
        // fit inside a test run, so that one is not a wait.
        429 if message.contains("PerDay") => {
            Upstream::Unavailable("the day's free-tier quota is spent".to_owned())
        }
        429 => Upstream::Transient {
            delay: retry_after(message, RATE_LIMIT_BACKOFF),
            reason: "rate limited (429)".to_owned(),
        },
        // Transient by definition, and Google's own message says so: "This
        // model is currently experiencing high demand. Spikes in demand are
        // usually temporary."
        500 | 502 | 503 | 504 => Upstream::Transient {
            delay: retry_after(message, BACKOFF * 2u32.pow(attempt - 1)),
            reason: format!("upstream capacity ({status})"),
        },
        404 => Upstream::Unavailable("no such model on this endpoint".to_owned()),
        // A 400 is a request this harness built wrong, a 401/403 is the key.
        // Either way, somebody needs to look at it.
        _ => Upstream::Ours,
    }
}

/// The HTTP status the agent reported, if it reported one at all.
fn status_of(message: &str) -> Option<u16> {
    message
        .split_once("returned HTTP ")?
        .1
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

/// What the provider asked to be waited, or `default` when it did not ask.
fn retry_after(message: &str, default: Duration) -> Duration {
    retry_info(message).unwrap_or(default).min(MAX_RETRY)
}

/// Digs `details[].RetryInfo.retryDelay` out of the embedded provider body.
///
/// Gemini-shaped, and absent from every other provider's error body — which is
/// what `default` is for.
fn retry_info(message: &str) -> Option<Duration> {
    let start = message.find('{')?;
    // The body is followed by nothing, but a streaming parser is what reads a
    // JSON value out of the middle of a sentence without guessing where it ends.
    let body: Value = serde_json::Deserializer::from_str(&message[start..])
        .into_iter()
        .next()?
        .ok()?;

    let delay = body
        .pointer("/error/details")?
        .as_array()?
        .iter()
        .filter(|detail| {
            detail
                .get("@type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.contains("RetryInfo"))
        })
        .find_map(|detail| detail.get("retryDelay").and_then(Value::as_str))?;

    // Protobuf durations, so "39s" or "1.5s".
    let seconds: f64 = delay.strip_suffix('s')?.parse().ok()?;
    Some(Duration::from_secs_f64(seconds) + Duration::from_secs(1))
}

fn run_error(events: &[Event]) -> Option<&str> {
    events.iter().find_map(|event| match event {
        Event::RunError(payload) => Some(payload.message.as_str()),
        _ => None,
    })
}

fn types(events: &[Event]) -> Vec<EventType> {
    events.iter().map(Event::event_type).collect()
}

/// Every `TEXT_MESSAGE_CONTENT` delta, in order.
fn deltas(events: &[Event]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::TextMessageContent(payload) => Some(payload.delta.as_str()),
            _ => None,
        })
        .collect()
}

/// The sorting itself, checked against real bodies and without the network.
///
/// These are the only tests in this file that are not `#[ignore]`: they touch
/// nothing, and the alternative is finding out that a `503` is mishandled during
/// the next outage, which is exactly when nobody wants to be reading this file.
mod classification {
    use super::*;

    /// Verbatim, from a run that hit it.
    const HIGH_DEMAND: &str = r#"agent error: the model returned HTTP 503: {"error": {"code": 503, "message": "This model is currently experiencing high demand. Spikes in demand are usually temporary.", "status": "UNAVAILABLE"}}"#;

    /// Verbatim, trimmed to the two `details` entries that matter.
    const DAILY_QUOTA: &str = r#"agent error: the model returned HTTP 429: {"error": {"code": 429, "status": "RESOURCE_EXHAUSTED", "details": [{"@type": "type.googleapis.com/google.rpc.QuotaFailure", "violations": [{"quotaId": "GenerateRequestsPerDayPerProjectPerModel-FreeTier", "quotaValue": "20"}]}, {"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "58s"}]}}"#;

    const PER_MINUTE_QUOTA: &str = r#"agent error: the model returned HTTP 429: {"error": {"code": 429, "status": "RESOURCE_EXHAUSTED", "details": [{"@type": "type.googleapis.com/google.rpc.QuotaFailure", "violations": [{"quotaId": "GenerateRequestsPerMinutePerProjectPerModel-FreeTier", "quotaValue": "10"}]}, {"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "39s"}]}}"#;

    #[test]
    fn high_demand_is_waited_out_not_reported_as_a_bug() {
        let Upstream::Transient { delay, reason } = classify(HIGH_DEMAND, 1) else {
            panic!("a 503 is transient by definition");
        };
        assert!(reason.contains("503"), "{reason}");
        // No RetryInfo in a 503 body, so the harness picks the back-off, and it
        // grows with each attempt.
        assert_eq!(delay, BACKOFF);
        let Upstream::Transient { delay, .. } = classify(HIGH_DEMAND, 3) else {
            panic!("still transient on a later attempt");
        };
        assert_eq!(delay, BACKOFF * 4);
    }

    #[test]
    fn a_per_minute_quota_waits_exactly_as_long_as_retry_info_asks() {
        let Upstream::Transient { delay, .. } = classify(PER_MINUTE_QUOTA, 1) else {
            panic!("a per-minute quota refills");
        };
        // "39s", plus a second so the retry lands after the window, not on it.
        assert_eq!(delay, Duration::from_secs(40));
    }

    #[test]
    fn a_daily_quota_moves_to_another_model_however_long_retry_info_says() {
        let Upstream::Unavailable(reason) = classify(DAILY_QUOTA, 1) else {
            panic!("a day does not fit inside a test run, RetryInfo notwithstanding");
        };
        assert!(reason.contains("day"), "{reason}");
    }

    #[test]
    fn an_unknown_model_is_skipped_over_rather_than_retried() {
        let message = r#"agent error: the model returned HTTP 404: {"error": {"code": 404, "message": "models/nope is not found", "status": "NOT_FOUND"}}"#;
        assert!(matches!(classify(message, 1), Upstream::Unavailable(_)));
    }

    /// Pointing the harness at a local server that is not running is a
    /// configuration mistake, not a mapping bug.
    #[test]
    fn an_endpoint_that_is_not_up_is_skipped_rather_than_failed() {
        let message = "agent error: error sending request for url (http://localhost:11434/v1/chat/completions)";
        let Upstream::Unavailable(reason) = classify(message, 1) else {
            panic!("an unreachable endpoint says nothing about the mapping");
        };
        assert!(reason.contains("reached"), "{reason}");
    }

    /// The half that has to stay loud: nothing upstream refused anything.
    #[test]
    fn our_own_failures_are_still_ours() {
        let malformed = r#"agent error: the model returned HTTP 400: {"error": {"code": 400, "message": "Invalid JSON payload", "status": "INVALID_ARGUMENT"}}"#;
        assert!(matches!(classify(malformed, 1), Upstream::Ours));

        for message in [
            "agent error: the model asked for tools 4 turns running",
            "agent error: the model sent a frame this agent could not read: EOF",
            "TEXT_MESSAGE_CONTENT breaks rule `not-open`",
        ] {
            assert!(
                matches!(classify(message, 1), Upstream::Ours),
                "{message} is not the provider's doing"
            );
        }
    }
}

/// The stream in one line per event, for a failure message worth reading.
fn summary(events: &[Event]) -> String {
    events
        .iter()
        .map(|event| match event {
            Event::TextMessageContent(payload) => {
                format!("  TEXT_MESSAGE_CONTENT {:?}", payload.delta)
            }
            Event::ToolCallStart(payload) => {
                format!("  TOOL_CALL_START {}", payload.tool_call_name)
            }
            Event::ToolCallArgs(payload) => format!("  TOOL_CALL_ARGS {}", payload.delta),
            Event::ToolCallResult(payload) => format!("  TOOL_CALL_RESULT {}", payload.content),
            Event::RunError(payload) => format!("  RUN_ERROR {}", payload.message),
            other => format!("  {}", other.event_type()),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A supervisor that delegates the whole answer to a child run of the model,
/// then signs off itself.
///
/// The child is the same [`LlmAgent`] run through a subagent handle: the
/// handle dereferences to the run context, so nothing about the agent knows
/// it was delegated to, and every event it emits comes out attributed by the
/// sink.
struct Delegating(LlmAgent);

impl Agent for Delegating {
    type State = ();

    async fn run(&self, ctx: &mut RunContext<()>) -> Result<RunOutcome> {
        let mut researcher = ctx.subagent("researcher")?;
        let outcome = self.0.run(&mut researcher).await?;
        match &outcome {
            RunOutcome::Success => researcher.finish()?,
            RunOutcome::Interrupt { interrupts } => {
                let ids: Vec<String> = interrupts.iter().map(|i| i.id.clone()).collect();
                researcher.suspend(ids)?;
                return Ok(outcome);
            }
        }
        ctx.say("Delegated.")?;
        Ok(outcome)
    }
}

/// A real model's stream, delegated: everything it produced arrives inside
/// `SUBAGENT_STARTED` / `SUBAGENT_FINISHED` and carries the invocation's id,
/// the supervisor's own sign-off does not, and the client files each where it
/// belongs.
#[tokio::test]
#[ignore = "spends live model quota; run with --ignored"]
async fn a_delegated_answer_arrives_attributed_to_the_subagent() {
    let Some(live) = live_with(Delegating).await else {
        return;
    };
    let _serialized = LIVE.lock().await;

    let Some(events) = run(
        &live,
        RunParams::new("live-thread", "live-subagent")
            .user("m1", "Reply with the single word: pong"),
    )
    .await
    else {
        return;
    };

    verify_all(&events).unwrap_or_else(|error| panic!("{error}\n{}", summary(&events)));

    let types = types(&events);
    let opened = types
        .iter()
        .position(|kind| *kind == EventType::SubagentStarted)
        .unwrap_or_else(|| panic!("no SUBAGENT_STARTED: {types:?}"));
    let closed = types
        .iter()
        .position(|kind| *kind == EventType::SubagentFinished)
        .unwrap_or_else(|| panic!("no SUBAGENT_FINISHED: {types:?}"));
    assert!(opened < closed, "{types:?}");

    let Event::SubagentStarted(started) = &events[opened] else {
        unreachable!("found by type");
    };
    let id = &started.subagent_run_id;
    assert_eq!(started.name, "researcher");

    // The model's own stream, every event of it, is the subagent's — the
    // sink tagged what the agent never knew it was emitting under a scope.
    let inside = &events[opened + 1..closed];
    assert!(!inside.is_empty(), "{types:?}");
    assert!(
        inside
            .iter()
            .all(|event| event.subagent_run_id() == Some(id)),
        "an event inside the scope is untagged:\n{}",
        summary(&events)
    );
    assert!(
        inside
            .iter()
            .any(|event| event.event_type() == EventType::TextMessageContent),
        "the model said nothing inside the scope:\n{}",
        summary(&events)
    );

    // The sign-off after the scope is the supervisor's own.
    let after = &events[closed + 1..];
    assert!(
        after
            .iter()
            .filter(|event| event.event_type() != EventType::RunFinished)
            .all(|event| event.subagent_run_id().is_none()),
        "{}",
        summary(&events)
    );

    // And the consuming side files it all: the registry closes the
    // invocation, the model's text is on a message attributed to it, and the
    // sign-off is on one that is not.
    let mut applier = Applier::new();
    for event in &events {
        applier
            .apply(event)
            .unwrap_or_else(|error| panic!("{error}\n{}", summary(&events)));
    }
    let subagent = applier.subagent(id).expect("the invocation is registered");
    assert!(
        matches!(
            subagent.status,
            ag_ui::client::SubagentStatus::Finished { .. }
        ),
        "{:?}",
        subagent.status
    );
    let said: Vec<(Option<&str>, String)> = applier
        .messages()
        .iter()
        .filter_map(|message| match message {
            ag_ui::Message::Assistant(assistant) => Some((
                message.subagent_run_id().map(|id| id.as_str()),
                assistant.content.clone().unwrap_or_default(),
            )),
            _ => None,
        })
        .collect();
    let delegated: Vec<&str> = said
        .iter()
        .filter(|(owner, _)| *owner == Some(id.as_str()))
        .map(|(_, text)| text.as_str())
        .collect();
    assert!(
        delegated.iter().any(|text| !text.trim().is_empty()),
        "no attributed reply: {said:?}"
    );
    assert!(
        said.iter()
            .any(|(owner, text)| owner.is_none() && text == "Delegated."),
        "the supervisor's sign-off is missing or misattributed: {said:?}"
    );
    eprintln!("live delegated reply: {delegated:?} under {}", id.as_str());
}
