//! cedar: a backend service for agent runs.
//!
//! Run:  CEDAR_API_KEYS=devkey cargo run   (needs an OpenAI-compatible LLM server, e.g. vLLM)
//! See README.md for the HTTP API and configuration.

use anyhow::Result;
use cedar::config::Config;
use cedar::server::{serve, Extensions};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // For scripts/cedar-update.sh, which compares the installed version with the release's.
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("cedar {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    // The driver of this computer's own screen, which the server starts once per `computer`
    // call when `COMPUTER_RUNTIME=native` (see cedar::computer and cedar::native).
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("computer") {
        #[cfg(feature = "native-computer")]
        std::process::exit(cedar::native::main(&args[1..]));
        #[cfg(not(feature = "native-computer"))]
        {
            eprintln!("this cedar was built without the native computer driver (the `native-computer` feature)");
            std::process::exit(1);
        }
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn")),
        )
        .init();

    let cfg = Config::from_env()?;
    serve(cfg, Extensions::default()).await
}
