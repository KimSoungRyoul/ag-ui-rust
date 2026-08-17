//! Serves a real, streaming LLM over AG-UI.
//!
//! Any OpenAI-compatible `/chat/completions` endpoint works. With no
//! configuration it talks to Gemini's compatibility endpoint:
//!
//! ```text
//! export GEMINI_API_KEY=…            # https://aistudio.google.com/apikey
//! cargo run -p ag-ui-e2e --example llm_agent
//! ```
//!
//! Against a model on your own machine it needs no key at all:
//!
//! ```text
//! ollama serve && ollama pull qwen3:4b
//! export AG_UI_LLM_BASE_URL=http://localhost:11434/v1
//! export AG_UI_LLM_MODEL=qwen3:4b
//! cargo run -p ag-ui-e2e --example llm_agent
//! ```
//!
//! Then point a browser client at `http://127.0.0.1:8080/agent`, or POST a run
//! yourself — the endpoint answers with `text/event-stream`, so `curl -N` shows
//! the events arriving one by one. Set `ADDR` to bind somewhere else.
//!
//! The agent is [`ag_ui_e2e::llm::LlmAgent`], which reaches the model with
//! `reqwest` and implements nothing but [`Agent`](ag_ui_server::Agent). Mounting
//! it is one line, and there is no LLM crate anywhere in the dependency tree —
//! that is the point of the example as much as the streaming is.

use ag_ui_axum::RouterExt;
use ag_ui_e2e::llm::{API_KEY_ENV, BASE_URL_ENV, LlmAgent, MODEL_ENV};
use axum::Router;
use axum::routing::get;

#[tokio::main]
async fn main() {
    // No panic on a missing key: this is the first thing a reader runs, and a
    // backtrace is a poor way to be told to export a variable.
    let agent = match LlmAgent::from_env() {
        Ok(agent) => agent,
        Err(error) => {
            eprintln!("{error}.");
            eprintln!();
            eprintln!("Either get a key at https://aistudio.google.com/apikey, then:");
            eprintln!("    export {API_KEY_ENV}=…");
            eprintln!("or point this at a model on your own machine:");
            eprintln!("    export {BASE_URL_ENV}=http://localhost:11434/v1");
            eprintln!("    export {MODEL_ENV}=qwen3:4b");
            std::process::exit(1);
        }
    };

    let (model, endpoint) = (agent.model_name().to_owned(), agent.base_url().to_owned());
    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route_agui("/agent", agent);

    let addr = std::env::var("ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_owned());
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("could not bind {addr}: {error}");
            std::process::exit(1);
        }
    };
    let local = listener
        .local_addr()
        .map_or_else(|_| addr.clone(), |addr| addr.to_string());

    // The endpoint, never the key: the key is a header, and this line is the
    // first thing that would leak it.
    println!("{model} on AG-UI, via {endpoint}");
    println!("  POST http://{local}/agent    run endpoint (text/event-stream)");
    println!("  GET  http://{local}/health");
    println!();
    let body = r#"{"threadId":"t1","runId":"r1","messages":[{"role":"user","id":"m1","content":"What is the weather in Seoul?"}],"tools":[],"context":[],"state":{}}"#;
    println!("Ask it something:");
    println!("  curl -N http://{local}/agent \\");
    println!("    -H 'content-type: application/json' \\");
    println!("    -d '{body}'");
    println!();
    println!("That prompt reaches the agent's own get_weather tool, so the stream carries");
    println!("TOOL_CALL_START / _ARGS / _END / _RESULT before the final answer.");

    if let Err(error) = axum::serve(listener, app).await {
        eprintln!("server stopped: {error}");
        std::process::exit(1);
    }
}
