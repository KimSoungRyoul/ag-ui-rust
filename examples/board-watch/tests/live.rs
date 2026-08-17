//! The client against a real streaming model.
//!
//! ```text
//! export GEMINI_API_KEY=…            # or AG_UI_LLM_API_KEY
//! cargo test -p board-watch --test live -- --ignored --nocapture
//! ```
//!
//! Or against a model on your own machine, where no key is needed at all:
//!
//! ```text
//! ollama serve && ollama pull qwen3:4b
//! export AG_UI_LLM_BASE_URL=http://localhost:11434/v1
//! export AG_UI_LLM_MODEL=qwen3:4b
//! cargo test -p board-watch --test live -- --ignored --nocapture
//! ```
//!
//! `#[ignore]`, so `cargo test` and CI never touch the network, and it skips
//! rather than fails when there is no key — the same arrangement as
//! `e2e/tests/live_llm.rs`, and for the same reason: a contributor with no key
//! should still see a green run.
//!
//! # What it proves that the fake backend cannot
//!
//! [`board_watch::fake`] fragments streams the way this file's author *expects*
//! a provider to. This one is fragmented by an actual provider, on its own
//! schedule, across real TCP segments — so the assertion that matters is the
//! dull one: however the deltas landed, the client assembled exactly one
//! message out of them.

use ag_ui_client::Session;
use ag_ui_client::transport::HttpTransport;
use ag_ui_core::Message;
use board_watch::Board;
use board_watch::watch::{Console, Watch};
use tokio::net::TcpListener;

/// Serves `agent` on a free loopback port and returns its URL.
async fn serve(agent: ag_ui_e2e::llm::LlmAgent) -> String {
    use ag_ui_axum::RouterExt;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free port on loopback");
    let addr = listener.local_addr().expect("the bound address");
    let app = axum::Router::new().route_agui("/agent", agent);

    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("the server to run");
    });
    format!("http://{addr}/agent")
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "talks to a real model; needs AG_UI_LLM_API_KEY, GEMINI_API_KEY or AG_UI_LLM_BASE_URL"]
async fn a_real_model_stream_assembles_into_one_message() {
    let agent = match ag_ui_e2e::llm::LlmAgent::from_env() {
        Ok(agent) => agent,
        Err(reason) => {
            println!("skipping: {reason}");
            return;
        }
    };
    println!("asking {} via {}", agent.model_name(), agent.base_url());

    let url = serve(agent).await;
    let mut session: Session<_, Board> = Session::new(
        HttpTransport::new(&url).expect("a valid endpoint URL"),
        "live",
    );

    let mut console = Console::new(
        "Reply with exactly this sentence and nothing else: the board is ready.\n".as_bytes(),
        Vec::new(),
    )
    .echoing();
    board_watch::watch::watch(
        &mut session,
        Watch {
            fragments: true,
            ..Watch::default()
        },
        &mut console,
    )
    .await
    .expect("a Vec never fails to be written to");

    let printed = String::from_utf8(console.into_output()).expect("UTF-8");
    println!("{printed}");

    // Someone else's capacity-constrained service: a failed run says nothing
    // about this client, so it skips rather than fails. A stream is asserted on.
    if printed.contains("  done   failed") {
        println!("skipping: the model did not answer");
        return;
    }

    let replies: Vec<&str> = session
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::Assistant(assistant) => assistant.content.as_deref(),
            _ => None,
        })
        .collect();

    assert_eq!(replies.len(), 1, "one message, however it was fragmented");
    assert!(!replies[0].trim().is_empty(), "{printed}");
    assert!(printed.contains("  done   success"), "{printed}");
    // `][` is the seam between two deltas. More than one means the reply really
    // did arrive in pieces, which is the only part of this a fixture cannot fake.
    assert!(
        printed.contains("]["),
        "the model should have streamed more than one delta:\n{printed}"
    );
}
