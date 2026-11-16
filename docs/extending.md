# Building on cedar from another repository

Cedar is a library as well as a binary. A separate crate, such as a closed-source enterprise edition or an in-house build, can depend on it and ship its own binary that adds features. That keeps the extra code out of this repository without a fork to rebase.

## The crate

```toml
# cedar-enterprise/Cargo.toml
[package]
name = "cedar-enterprise"
version = "0.10.1+ee.1"   # the cedar version it builds on, then its own counter
edition = "2021"

[dependencies]
cedar = { git = "https://github.com/hanneshapke/cedar", tag = "v0.10.1" }
anyhow = "1"
axum = "0.8"              # the same major versions cedar uses
sqlx = { version = "0.8", default-features = false, features = ["sqlite", "migrate", "macros"] }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

```rust
// cedar-enterprise/src/main.rs
use axum::{routing::get, Router};
use cedar::server::{serve, Extensions};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("info,sqlx=warn"))
        .init();
    let cfg = cedar::config::Config::from_env()?;

    let ext = Extensions::default()
        // Under /v1, behind cedar's API-key check.
        .v1_routes(Router::new().route("/ee/audit", get(audit::list)))
        // At the top level, without it (e.g. an SSO callback).
        .public_routes(Router::new().route("/sso/callback", get(sso::callback)))
        // Middleware around the whole router; the last one added is outermost.
        .layer(|r| r.layer(axum::middleware::from_fn(audit::record)))
        // Runs before interrupted runs are recovered and before the server listens.
        .on_start(|state| async move {
            state
                .store
                .run_extension_migrations(sqlx::migrate!("./migrations"))
                .await?;
            tokio::spawn(reports::nightly(state.clone())); // background work: spawn, don't block
            Ok(())
        });

    serve(cfg, ext).await
}
```

Handlers get cedar's `AppState` (`State<AppState>`) like its own: the store, the run manager, the event hub and the integrations.

## Migrations

Extension migrations live in the extension crate's own `migrations/` directory and share sqlx's bookkeeping table with cedar's. So:

- Number them from `1000000` up (`1000000_ee_audit.sql`, `1000001_ee_sso.sql`, …). `Store::run_extension_migrations` refuses anything lower, which is the range cedar uses for its own.
- Create your own tables, with a prefix such as `ee_`, and query them through `state.store.pool()`. Don't alter cedar's tables: a later cedar migration may change them under you. If you need a column on one of cedar's tables, add the column upstream.
- Cedar ignores applied versions it doesn't know, so a database can go back to the open-source binary. The extension tables are then simply unused.

## Workflow

1. **Seams go upstream, features stay downstream.** When a feature needs a hook cedar lacks (an auth provider, an event sink), add the generic hook to cedar in a normal pull request and keep only the implementation in the extension crate.
2. **Pin to a release tag.** Let Renovate or Dependabot open a pull request in the extension repository for each new cedar tag, and keep its CI running the extension's tests on that pull request.
3. **Build nightly against `main`.** A scheduled CI job builds the extension against cedar's `main` (`cargo update -p cedar` with the dependency on `branch = "main"`, or a `[patch]`), so a breaking change shows up before the release, not after.
4. **Develop both at once** with an uncommitted override pointing at a sibling checkout:

   ```toml
   # cedar-enterprise/.cargo/config.toml (in .gitignore)
   [patch."https://github.com/hanneshapke/cedar"]
   cedar = { path = "../cedar" }
   ```

5. **Reuse cedar's test fakes.** `tests/common/mod.rs` is not part of the library. Copy it into the extension's tests, or move the fakes upstream into a `cedar-testing` crate once a second consumer needs them, and build test apps with `cedar::api::router_with`.

## Clients

The web chat, desktop and mobile apps talk to whichever server they are given, so an extension build doesn't need its own clients. If a client should show an extension's features, add a generic way for the server to announce them upstream (for example a capabilities list in `GET /v1/defaults`) rather than forking the client.
