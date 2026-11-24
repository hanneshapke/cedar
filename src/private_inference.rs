//! `cedar private-inference connect | disconnect`: Cedar Private Inference for a server without
//! a browser. `connect` prints a code and the page to approve it on (in a browser anywhere),
//! waits, and registers the two models with the running cedar through its admin API, the way the
//! desktop app and the web chat do. `disconnect` removes them.
//!
//! The running server is found at `--cedar` (or `CEDAR_URL`), else at `BIND_ADDR` as it reads it;
//! the admin key is `--key` (or `CEDAR_ADMIN_KEY`), else the first of `CEDAR_ADMIN_KEYS`, so on
//! the server itself, with its environment, `cedar private-inference connect` is enough.

use anyhow::{anyhow, bail, Context, Result};
use cedar_private_inference::{Challenge, Client, LARGE_ID, SMALL_ID};
use serde_json::Value;
use std::time::Duration;

const USAGE: &str = "usage: cedar private-inference <connect | disconnect> [--cedar URL] [--key ADMIN_KEY]

  connect      print a code to approve in a browser, then register Cedar Private Inference's
               models with the running cedar
  disconnect   remove those models (the key stays valid until revoked on the account page)

  --cedar URL  the running cedar (default: CEDAR_URL, else http://BIND_ADDR, else
               http://127.0.0.1:8080)
  --key KEY    an admin key (default: CEDAR_ADMIN_KEY, else the first of CEDAR_ADMIN_KEYS)

CEDAR_PRIVATE_INFERENCE_URL points it at another instance of the service.";

#[derive(Debug, PartialEq)]
enum Command {
    Connect,
    Disconnect,
}

#[derive(Debug, PartialEq)]
struct Options {
    command: Command,
    cedar: String,
    key: String,
}

/// Runs the subcommand (`args` after `private-inference`); the process's exit code.
pub async fn main(args: &[String]) -> i32 {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return 0;
    }
    let env = |name: &str| std::env::var(name).ok();
    let opts = match parse(args, env) {
        Ok(opts) => opts,
        Err(e) => {
            eprintln!("{e}\n\n{USAGE}");
            return 2;
        }
    };
    let result = match opts.command {
        Command::Connect => connect(&opts, cedar_private_inference::service_url().map_err(|e| anyhow!(e))).await,
        Command::Disconnect => disconnect(&opts).await,
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    }
}

fn parse(args: &[String], env: impl Fn(&str) -> Option<String>) -> Result<Options> {
    let mut command = None;
    let mut cedar = None;
    let mut key = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "connect" if command.is_none() => command = Some(Command::Connect),
            "disconnect" if command.is_none() => command = Some(Command::Disconnect),
            "--cedar" => cedar = Some(args.next().context("--cedar needs a URL")?.clone()),
            "--key" => key = Some(args.next().context("--key needs an admin key")?.clone()),
            other => bail!("unexpected argument {other:?}"),
        }
    }
    let command = command.context("say connect or disconnect")?;
    let cedar = cedar
        .or_else(|| env("CEDAR_URL"))
        .unwrap_or_else(|| {
            let bind = env("BIND_ADDR").unwrap_or_else(|| "127.0.0.1:8080".into());
            // A server listening everywhere is reached on this machine at loopback.
            let bind = bind.replace("0.0.0.0", "127.0.0.1").replace("[::]", "[::1]");
            format!("http://{bind}")
        })
        .trim_end_matches('/')
        .to_string();
    let key = key
        .or_else(|| env("CEDAR_ADMIN_KEY"))
        .or_else(|| env("CEDAR_ADMIN_KEYS").and_then(|keys| keys.split(',').map(str::trim).find(|k| !k.is_empty()).map(String::from)))
        .filter(|k| !k.trim().is_empty())
        .context("no admin key: pass --key, or set CEDAR_ADMIN_KEY or CEDAR_ADMIN_KEYS")?;
    Ok(Options { command, cedar, key })
}

fn http() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder().timeout(Duration::from_secs(60)).build()?)
}

async fn connect(opts: &Options, service: Result<String>) -> Result<()> {
    let service = service?;
    // Before the user approves anything: is cedar there, and does it take this key?
    let check = http()?
        .get(format!("{}/v1/admin/models", opts.cedar))
        .bearer_auth(&opts.key)
        .send()
        .await
        .with_context(|| format!("could not reach cedar at {}; is it running? (--cedar)", opts.cedar))?;
    if !check.status().is_success() {
        bail!("cedar at {} refused the admin key ({})", opts.cedar, check.status());
    }

    let client = Client::new(&service)?;
    let challenge = Challenge::new();
    let name = format!("Cedar server {}", hostname());
    let started = client.start(&challenge, Some(name.trim())).await?;
    println!("To connect Cedar Private Inference, open this page in a browser, sign in and approve the code:\n");
    println!("  {}\n", started.verification_uri_complete);
    println!("  Code: {}\n", started.user_code);
    println!("Waiting for the approval (up to {} minutes)…", started.expires_in / 60);
    let grant = client.connect(&challenge, &started).await?;
    println!("Approved. Registering the models with cedar…");
    for (id, spec) in grant.model_specs() {
        let saved = cedar_private_inference::retry_while_activating(|| put_model(opts, id, &spec)).await?;
        println!("  {id}: {}", saved["label"].as_str().unwrap_or(id));
    }
    println!("\nDone. Balance and top-up: {}", grant.account_url);
    Ok(())
}

async fn put_model(opts: &Options, id: &str, spec: &Value) -> Result<Value> {
    let resp = http()?
        .put(format!("{}/v1/admin/models/{id}", opts.cedar))
        .bearer_auth(&opts.key)
        .json(spec)
        .send()
        .await
        .with_context(|| format!("could not reach cedar at {}", opts.cedar))?;
    let status = resp.status();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    if !status.is_success() {
        let message = body["error"]["message"].as_str().unwrap_or("");
        bail!("cedar did not take {id} ({status}): {message}");
    }
    Ok(body)
}

async fn disconnect(opts: &Options) -> Result<()> {
    for id in [SMALL_ID, LARGE_ID] {
        let resp = http()?
            .delete(format!("{}/v1/admin/models/{id}", opts.cedar))
            .bearer_auth(&opts.key)
            .send()
            .await
            .with_context(|| format!("could not reach cedar at {}; is it running? (--cedar)", opts.cedar))?;
        match resp.status() {
            s if s.is_success() => println!("removed {id}"),
            reqwest::StatusCode::NOT_FOUND => println!("{id} was not registered"),
            s => bail!("cedar did not remove {id} ({s})"),
        }
    }
    println!("The key stays valid until you revoke it on the account page.");
    Ok(())
}

fn hostname() -> String {
    ["HOSTNAME", "COMPUTERNAME"]
        .into_iter()
        .find_map(|v| std::env::var(v).ok())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok())
        .map(|h| h.trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// What the stub cedar was sent: `("put <id>" | "delete <id>", body)`.
    type Seen = Arc<Mutex<Vec<(String, Value)>>>;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn options_come_from_flags_then_the_servers_own_environment() {
        let none = |_: &str| None;
        let env = |name: &str| match name {
            "BIND_ADDR" => Some("0.0.0.0:9000".to_string()),
            "CEDAR_ADMIN_KEYS" => Some(" ,admin-1, admin-2".to_string()),
            _ => None,
        };
        assert_eq!(
            parse(&args(&["connect"]), env).unwrap(),
            Options { command: Command::Connect, cedar: "http://127.0.0.1:9000".into(), key: "admin-1".into() }
        );
        assert_eq!(
            parse(&args(&["disconnect", "--cedar", "https://cedar.example/", "--key", "k"]), none).unwrap(),
            Options { command: Command::Disconnect, cedar: "https://cedar.example".into(), key: "k".into() }
        );
        assert!(parse(&args(&["connect"]), none).is_err(), "no admin key");
        assert!(parse(&args(&["--key", "k"]), none).is_err(), "no command");
        assert!(parse(&args(&["connect", "now", "--key", "k"]), none).is_err());
        assert!(parse(&args(&["connect", "--cedar"]), none).is_err());
    }

    /// A cedar admin API that keeps what it was sent, and a cedar-plg that grants at once.
    async fn stubs() -> (String, String, Seen) {
        use axum::extract::{Path, State};
        use axum::routing::{get, post, put};
        use axum::Json;
        let seen: Seen = Arc::default();
        let cedar = axum::Router::new()
            .route("/v1/admin/models", get(|| async { Json(serde_json::json!({ "models": [] })) }))
            .route(
                "/v1/admin/models/{id}",
                put(|State(seen): State<Seen>, Path(id): Path<String>, Json(body): Json<Value>| async move {
                    seen.lock().unwrap().push((format!("put {id}"), body.clone()));
                    Json(serde_json::json!({ "id": id, "label": body["label"] }))
                })
                .delete(|State(seen): State<Seen>, Path(id): Path<String>| async move {
                    seen.lock().unwrap().push((format!("delete {id}"), Value::Null));
                    if id == LARGE_ID { axum::http::StatusCode::NOT_FOUND } else { axum::http::StatusCode::NO_CONTENT }
                }),
            )
            .with_state(seen.clone());
        let plg = axum::Router::new()
            .route(
                "/api/connect/start",
                post(|| async {
                    Json(serde_json::json!({ "user_code": "AAAA-BBBB", "verification_uri": "http://plg.test/connect",
                        "verification_uri_complete": "http://plg.test/connect?challenge=x", "expires_in": 900, "interval": 0 }))
                }),
            )
            .route(
                "/api/connect/poll",
                post(|| async {
                    Json(serde_json::json!({ "base_url": "https://api.example/v1", "api_key": "sk", "provider": "p",
                        "account_url": "http://plg.test/account",
                        "models": [{ "id": "small-model", "name": "Small 8B", "role": "small", "params_b": 8 }] }))
                }),
            );
        let serve = |app: axum::Router| async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            format!("http://{addr}")
        };
        (serve(cedar).await, serve(plg).await, seen)
    }

    #[tokio::test]
    async fn connect_registers_the_grant_and_disconnect_removes_it() {
        let (cedar, plg, seen) = stubs().await;
        let opts = Options { command: Command::Connect, cedar, key: "admin".into() };
        connect(&opts, Ok(plg)).await.unwrap();
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 1, "a grant with one model registers one");
            assert_eq!(seen[0].0, "put private-small");
            assert_eq!(seen[0].1["service"], "cedar-private-inference");
            assert_eq!(seen[0].1["is_default"], true);
        }
        disconnect(&opts).await.unwrap();
        let routes: Vec<String> = seen.lock().unwrap().iter().map(|(r, _)| r.clone()).collect();
        assert_eq!(routes[1..], ["delete private-small".to_string(), "delete private-large".to_string()]);

        // Without a running cedar, nothing is started at the service.
        let gone = Options { cedar: "http://127.0.0.1:1".into(), ..opts };
        let err = connect(&gone, Ok("http://127.0.0.1:1".into())).await.unwrap_err();
        assert!(format!("{err:#}").contains("is it running"), "{err:#}");
    }
}
