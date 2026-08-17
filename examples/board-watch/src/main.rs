//! The CLI.
//!
//! ```text
//! board-watch watch      [--url URL] [--thread ID] [--approve|--decline]
//!                        [--fragments] [--in-order] [--no-verify] [--stop-after N]
//! board-watch trace      [--url URL] [--thread ID] [--tools FILE] [--approve] SAID
//! board-watch replay     FIXTURE [--fragments]
//! board-watch serve-fake [--port 8090]
//! ```
//!
//! Argument parsing is by hand, as in `task-board`: a dozen lines of `match`
//! beats adding a dependency to an example whose point is what the SDK needs
//! and nothing else.

use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

use ag_ui_client::transport::HttpTransport;
use ag_ui_client::{HttpAgent, Session};
use board_watch::watch::{Console, Policy, Watch};
use board_watch::{Board, fake, load_tools, replay_fixture, trace, watch};

/// Where `watch` and `trace` look unless told otherwise — `task-board serve`.
const DEFAULT_URL: &str = "http://127.0.0.1:8080/agent";
/// The thread they join unless told otherwise.
const DEFAULT_THREAD: &str = "watch";
/// Where `serve-fake` binds unless told otherwise. Deliberately not 8080: the
/// interesting configuration is this *and* `task-board serve` at once.
const DEFAULT_PORT: u16 = 8090;

#[tokio::main]
async fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("watch") => run_watch(args).await,
        Some("trace") => run_trace(args).await,
        Some("replay") => run_replay(args).await,
        Some("serve-fake") => run_serve(args).await,
        Some("--help" | "-h") | None => match usage(&mut io::stdout()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => fail(&format!("{error}")),
        },
        Some(other) => {
            eprintln!("board-watch: unknown command \"{other}\"");
            let _ = usage(&mut io::stderr());
            ExitCode::FAILURE
        }
    }
}

/// `watch` — the application.
async fn run_watch(mut args: impl Iterator<Item = String>) -> ExitCode {
    let mut url = DEFAULT_URL.to_owned();
    let mut thread = DEFAULT_THREAD.to_owned();
    let mut settings = Watch::default();
    let mut verify = true;
    let mut tools_path = None;

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
            "--approve" => settings.policy = Policy::Approve,
            "--decline" => settings.policy = Policy::Decline,
            "--fragments" => settings.fragments = true,
            "--in-order" => settings.in_order = true,
            "--no-verify" => verify = false,
            "--tools" => match args.next() {
                Some(value) => tools_path = Some(value),
                None => return fail("--tools needs a path to a JSON array of tools"),
            },
            "--stop-after" => match args.next().as_deref().map(str::parse) {
                Some(Ok(value)) => settings.stop_after = Some(value),
                _ => return fail("--stop-after needs a number of updates"),
            },
            other => return fail(&format!("watch: unexpected argument \"{other}\"")),
        }
    }

    let tools = match read_tools(tools_path.as_deref()) {
        Ok(tools) => tools,
        Err(message) => return fail(&message),
    };

    let transport = match HttpTransport::new(&url) {
        Ok(transport) => transport,
        Err(error) => return fail(&format!("{url} is not a usable endpoint: {error}")),
    };
    let mut session: Session<_, Board> = Session::builder(transport, thread.clone())
        .tools(tools.clone())
        .verify(verify)
        .build();

    let mut console = console();
    let _ = writeln!(
        console,
        "board-watch · {url} · thread {thread} · verify {} · interrupts {} · {} tools",
        if verify { "on" } else { "off" },
        policy_name(settings.policy),
        tools.len(),
    );

    match watch::watch(&mut session, settings, &mut console).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(&format!("the terminal went away: {error}")),
    }
}

/// `trace` — the same conversation, unassembled.
async fn run_trace(mut args: impl Iterator<Item = String>) -> ExitCode {
    let mut url = DEFAULT_URL.to_owned();
    let mut thread = DEFAULT_THREAD.to_owned();
    let mut approve = false;
    let mut tools_path = None;
    let mut said = Vec::new();

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
            "--tools" => match args.next() {
                Some(value) => tools_path = Some(value),
                None => return fail("--tools needs a path to a JSON array of tools"),
            },
            "--approve" => approve = true,
            other => said.push(other.to_owned()),
        }
    }
    if said.is_empty() {
        return fail("trace needs something to say");
    }

    let tools = match read_tools(tools_path.as_deref()) {
        Ok(tools) => tools,
        Err(message) => return fail(&message),
    };

    let agent = match HttpAgent::builder(&url)
        // Proves the header survives to the agent's `RunAgentInput` request —
        // and that a credential would ride here, never in the query string.
        .header("x-board-watch", "trace")
        .build()
    {
        Ok(agent) => agent,
        Err(error) => return fail(&format!("{url} is not a usable endpoint: {error}")),
    };

    let mut out = io::stdout().lock();
    match trace::trace(&agent, &thread, &said.join(" "), tools, approve, &mut out).await {
        Ok(count) => {
            let _ = writeln!(out, "--- {count} events");
            ExitCode::SUCCESS
        }
        Err(error) => fail(&format!("{error}")),
    }
}

/// `replay` — the client with the network taken out.
async fn run_replay(args: impl Iterator<Item = String>) -> ExitCode {
    let mut path = None;
    let mut settings = Watch::default();

    for arg in args {
        match arg.as_str() {
            "--fragments" => settings.fragments = true,
            "--in-order" => settings.in_order = true,
            other => path = Some(other.to_owned()),
        }
    }
    let Some(path) = path else {
        return fail("replay needs a fixture path");
    };

    let json = match std::fs::read_to_string(&path) {
        Ok(json) => json,
        Err(error) => return fail(&format!("could not read {path}: {error}")),
    };
    let transport = match replay_fixture(&json) {
        Ok(transport) => transport,
        Err(error) => return fail(&format!("{path} is not a run fixture: {error}")),
    };

    let mut session: Session<_, Board> = Session::new(transport, "replay");
    let mut console = console();
    let _ = writeln!(console, "board-watch · replaying {path}");

    match watch::watch(&mut session, settings, &mut console).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(&format!("the terminal went away: {error}")),
    }
}

/// `serve-fake` — the backend the transcripts are recorded against.
async fn run_serve(mut args: impl Iterator<Item = String>) -> ExitCode {
    let mut port = DEFAULT_PORT;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--port" => match args.next().as_deref().map(str::parse) {
                Some(Ok(value)) => port = value,
                _ => return fail("--port needs a port number"),
            },
            other => return fail(&format!("serve-fake: unexpected argument \"{other}\"")),
        }
    }

    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => listener,
        Err(error) => return fail(&format!("could not bind port {port}: {error}")),
    };
    let addr = match listener.local_addr() {
        Ok(addr) => addr.to_string(),
        Err(error) => return fail(&format!("could not read the bound address: {error}")),
    };

    println!("the awkward agent, for pointing a client at");
    println!("  POST http://{addr}{}", fake::ROUTE);
    println!("  POST http://{addr}/raw/{{unbracketed|truncated|orphan-result}}");
    println!();
    println!("Scenarios are the first word of the message:");
    println!("  chunks   text as TEXT_MESSAGE_CHUNK, id on the first only");
    println!("  call     tool arguments split mid-escape");
    println!("  parallel two calls in flight, events interleaved");
    println!("  mixed    reasoning, text and a call, none of them bracketed");
    println!("  approve  pauses on two decisions at once");
    println!("  busy     does work, then pauses — both halves in one run");
    println!("  slow     never finishes — for --stop-after");
    println!("  fail     ends as RUN_ERROR");
    println!();
    println!("In another terminal:");
    println!(
        "  cargo run -p board-watch -- watch --url http://{addr}{} --fragments",
        fake::ROUTE
    );

    match axum::serve(listener, fake::router(fake::Awkward::new())).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(&format!("the server stopped: {error}")),
    }
}

/// Reads the tool list a run offers, or an empty one.
///
/// The agent only ever sees the tools this client sends, and nothing in the
/// protocol lets it ask for more. See [`board_watch::load_tools`].
fn read_tools(path: Option<&str>) -> Result<Vec<ag_ui_core::Tool>, String> {
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let json =
        std::fs::read_to_string(path).map_err(|error| format!("could not read {path}: {error}"))?;
    load_tools(&json).map_err(|error| format!("{path} is not a tool list: {error}"))
}

/// A console over the real terminal, echoing only when the script is piped.
fn console() -> Console<io::StdinLock<'static>, io::StdoutLock<'static>> {
    let stdin = io::stdin();
    let echo = !stdin.is_terminal();
    let console = Console::new(stdin.lock(), io::stdout().lock());
    if echo { console.echoing() } else { console }
}

fn policy_name(policy: Policy) -> &'static str {
    match policy {
        Policy::Ask => "ask",
        Policy::Approve => "approve",
        Policy::Decline => "decline",
    }
}

/// Prints `message` and fails.
fn fail(message: &str) -> ExitCode {
    eprintln!("board-watch: {message}");
    ExitCode::FAILURE
}

fn usage(out: &mut impl Write) -> io::Result<()> {
    writeln!(out, "board-watch — a terminal client for an AG-UI agent")?;
    writeln!(out)?;
    writeln!(
        out,
        "  board-watch watch      [--url {DEFAULT_URL}] [--thread {DEFAULT_THREAD}]"
    )?;
    writeln!(
        out,
        "                         [--approve|--decline] [--fragments] [--no-verify]"
    )?;
    writeln!(
        out,
        "                         [--tools FILE] [--in-order] [--stop-after N]"
    )?;
    writeln!(
        out,
        "  board-watch trace      [--url URL] [--thread ID] [--approve] SAID"
    )?;
    writeln!(out, "  board-watch replay     FIXTURE [--fragments]")?;
    writeln!(out, "  board-watch serve-fake [--port {DEFAULT_PORT}]")?;
    writeln!(out)?;
    writeln!(out, "No keys, no network beyond loopback.")
}
