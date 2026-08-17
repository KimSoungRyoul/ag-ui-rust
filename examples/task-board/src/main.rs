//! The CLI: `serve` hosts the agent, `chat` talks to it.
//!
//! ```text
//! task-board serve [--port 8080] [--llm]
//! task-board chat  [--url http://127.0.0.1:8080/agent] [--thread workshop]
//! ```
//!
//! Argument parsing is by hand. A dozen lines of `match` beats adding a
//! dependency to an example whose whole point is what the SDK needs and nothing
//! else.

use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

use ag_ui_client::Session;
use ag_ui_client::transport::HttpTransport;
use task_board::chat::Terminal;
use task_board::llm::Voice;
use task_board::{Board, ROUTE, TaskBoard, board, chat, router};

/// Where `chat` looks unless told otherwise.
const DEFAULT_URL: &str = "http://127.0.0.1:8080/agent";
/// The thread `chat` joins unless told otherwise. Reused on purpose: a second
/// `chat` against the same server continues the same conversation.
const DEFAULT_THREAD: &str = "workshop";
/// Where `serve` binds unless told otherwise.
const DEFAULT_PORT: u16 = 8080;

#[tokio::main]
async fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("serve") => serve(args).await,
        Some("chat") => chat(args).await,
        Some("--help" | "-h") | None => match usage(&mut io::stdout()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => fail(&format!("{error}")),
        },
        Some(other) => {
            eprintln!("task-board: unknown command \"{other}\"");
            let _ = usage(&mut io::stderr());
            ExitCode::FAILURE
        }
    }
}

/// `serve` — host the agent.
async fn serve(args: impl Iterator<Item = String>) -> ExitCode {
    let mut port = DEFAULT_PORT;
    let mut llm = false;
    let mut args = args;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--llm" => llm = true,
            "--port" => match args.next().as_deref().map(str::parse) {
                Some(Ok(value)) => port = value,
                _ => return fail("--port needs a port number"),
            },
            other => return fail(&format!("serve: unexpected argument \"{other}\"")),
        }
    }

    let agent = if llm {
        match Voice::from_env() {
            Ok(voice) => {
                // The endpoint, never the key: the key is a header, and this
                // line is the first thing that would leak it.
                println!(
                    "phrasing replies with {} via {}",
                    voice.model(),
                    voice.endpoint()
                );
                TaskBoard::with_voice(voice)
            }
            Err(error) => return fail(&format!("{error}")),
        }
    } else {
        TaskBoard::scripted()
    };

    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => listener,
        Err(error) => return fail(&format!("could not bind port {port}: {error}")),
    };
    let addr = match listener.local_addr() {
        Ok(addr) => addr.to_string(),
        Err(error) => return fail(&format!("could not read the bound address: {error}")),
    };

    println!("task board on AG-UI");
    println!("  POST http://{addr}{ROUTE}    run endpoint (text/event-stream)");
    println!("  GET  http://{addr}/health");
    println!();
    println!("In another terminal:");
    println!("  cargo run -p task-board -- chat --url http://{addr}{ROUTE}");

    match axum::serve(listener, router(agent)).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(&format!("the server stopped: {error}")),
    }
}

/// `chat` — talk to it.
async fn chat(args: impl Iterator<Item = String>) -> ExitCode {
    let mut url = DEFAULT_URL.to_owned();
    let mut thread = DEFAULT_THREAD.to_owned();
    let mut args = args;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--url" => match args.next() {
                Some(value) => url = value,
                None => return fail("--url needs an endpoint"),
            },
            "--thread" => match args.next() {
                Some(value) => thread = value,
                None => return fail("--thread needs an id"),
            },
            other => return fail(&format!("chat: unexpected argument \"{other}\"")),
        }
    }

    let transport = match HttpTransport::new(&url) {
        Ok(transport) => transport,
        Err(error) => return fail(&format!("{url} is not a usable endpoint: {error}")),
    };

    // The tools travel on every request; the agent reads them back out of
    // `ctx.tools()` and refuses to call one that is not there.
    let mut session = Session::<_, Board>::builder(transport, thread.clone())
        .tools(board::tools())
        .build();

    let stdin = io::stdin();
    // A script on a pipe has to be echoed for the transcript to read as a
    // conversation; a human has already seen what they typed.
    let echo = !stdin.is_terminal();
    let mut terminal = Terminal::new(stdin.lock(), io::stdout().lock());
    if echo {
        terminal = terminal.echoing();
    }

    let _ = writeln!(terminal, "task board · {url} · thread {thread}");
    let _ = writeln!(terminal, "try: add draft the agenda, book the room");
    match chat::converse(&mut session, &mut terminal).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(&format!("the terminal went away: {error}")),
    }
}

/// Prints `message` and fails.
fn fail(message: &str) -> ExitCode {
    eprintln!("task-board: {message}");
    ExitCode::FAILURE
}

fn usage(out: &mut impl Write) -> io::Result<()> {
    writeln!(out, "task-board — a workshop task board over AG-UI")?;
    writeln!(out)?;
    writeln!(out, "  task-board serve [--port {DEFAULT_PORT}] [--llm]")?;
    writeln!(
        out,
        "  task-board chat  [--url {DEFAULT_URL}] [--thread {DEFAULT_THREAD}]"
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "--llm phrases replies with an OpenAI-compatible model."
    )?;
    writeln!(
        out,
        "It needs AG_UI_LLM_API_KEY or GEMINI_API_KEY; the board never does."
    )
}
