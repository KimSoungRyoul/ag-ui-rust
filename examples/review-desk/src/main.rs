use std::io;

use review_desk::client::{AppResult, workflow};

#[tokio::main]
async fn main() -> AppResult<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str).unwrap_or("demo") {
        "demo" => {
            let (url, server) = review_desk::serve_local().await?;
            let result = workflow(&url, &mut io::stdout()).await;
            server.abort();
            let (evidence, snapshot) = result?;
            if let Some(path) = args.get(1) {
                std::fs::write(path, snapshot)?;
                println!("saved snapshot · {path}");
            }
            println!("{}", serde_json::to_string_pretty(&evidence)?);
        }
        "serve" => {
            let address = args.get(1).map(String::as_str).unwrap_or("127.0.0.1:8091");
            let listener = tokio::net::TcpListener::bind(address).await?;
            let base = format!("http://{}", listener.local_addr()?);
            println!("review-desk · {base}");
            axum::serve(listener, review_desk::router(format!("{base}/agent"))).await?;
        }
        "run" => {
            let url = args
                .get(1)
                .map(String::as_str)
                .unwrap_or("http://127.0.0.1:8091/agent");
            let (evidence, _) = workflow(url, &mut io::stdout()).await?;
            println!("{}", serde_json::to_string_pretty(&evidence)?);
        }
        "export-a2ui" => {
            println!(
                "{}",
                serde_json::to_string_pretty(&review_desk::interop::fixture().await?)?
            );
        }
        "--help" | "-h" => {
            println!(
                "review-desk demo [SNAPSHOT.json]\nreview-desk serve [127.0.0.1:8091]\nreview-desk run [http://127.0.0.1:8091/agent]\nreview-desk export-a2ui"
            );
        }
        other => return Err(format!("unknown command: {other}").into()),
    }
    Ok(())
}
