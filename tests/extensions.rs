//! The seams a downstream build (e.g. an enterprise edition in its own repository) extends cedar
//! through: extra routes (`api::router_with`, `server::Extensions`) and its own migrations
//! (`Store::run_extension_migrations`).

mod common;

use axum::extract::State;
use axum::http::{Method, StatusCode};
use axum::routing::get;
use axum::Router;
use cedar::api::{self, AppState};
use cedar::server::Extensions;
use cedar::store::{Store, EXTENSION_MIGRATION_FLOOR};
use common::*;
use sqlx::migrate::Migrator;
use std::path::PathBuf;
use uuid::Uuid;

#[tokio::test]
async fn extension_routes_are_mounted_and_v1_ones_need_a_key() {
    let mut app = TestApp::new(vec![]).await;
    let ext = Extensions::default()
        .v1_routes(Router::new().route(
            "/ee/whoami",
            // Extension handlers get cedar's state, like its own.
            get(|State(s): State<AppState>| async move { format!("{}", s.cfg.bind_addr) }),
        ))
        .public_routes(Router::new().route("/ee/ping", get(|| async { "pong" })));
    app.router = api::router_with(app.state.clone(), ext.v1_routes, ext.public_routes);

    let (status, _, body) = app.call(Method::GET, "/ee/ping", None, None).await;
    assert_eq!((status, body.as_str()), (StatusCode::OK, "pong"));

    let (status, _, _) = app.call(Method::GET, "/v1/ee/whoami", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, body) = app
        .call(Method::GET, "/v1/ee/whoami", Some(KEY_A), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.is_empty());

    // cedar's own routes are still there.
    let (status, _) = app.get("/v1/runs", KEY_A).await;
    assert_eq!(status, StatusCode::OK);
}

/// A directory holding the given migration files, as a downstream crate's `migrations/` would.
async fn migrations(files: &[(&str, &str)]) -> (PathBuf, Migrator) {
    let dir = std::env::temp_dir().join(format!("cedar-ext-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    for (name, sql) in files {
        std::fs::write(dir.join(name), sql).unwrap();
    }
    let migrator = Migrator::new(dir.as_path()).await.unwrap();
    (dir, migrator)
}

#[tokio::test]
async fn extension_migrations_coexist_with_cedars() {
    let db = std::env::temp_dir().join(format!("cedar-ext-{}.db", Uuid::new_v4()));
    let url = format!("sqlite://{}", db.display());
    let store = Store::connect(&url).await.unwrap();

    let (dir, migrator) = migrations(&[(
        &format!("{EXTENSION_MIGRATION_FLOOR}_ee_audit.sql"),
        "CREATE TABLE ee_audit (id INTEGER PRIMARY KEY, what TEXT NOT NULL);",
    )])
    .await;
    store.run_extension_migrations(migrator).await.unwrap();
    sqlx::query("INSERT INTO ee_audit (what) VALUES ('x')")
        .execute(store.pool())
        .await
        .unwrap();
    drop(store);

    // Restarting runs cedar's migrations first, which must not trip over the extension's.
    let store = Store::connect(&url).await.unwrap();
    // And applying the extension's again is a no-op.
    let migrator = Migrator::new(dir.as_path()).await.unwrap();
    store.run_extension_migrations(migrator).await.unwrap();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ee_audit")
        .fetch_one(store.pool())
        .await
        .unwrap();
    assert_eq!(n, 1);

    let _ = std::fs::remove_dir_all(dir);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", db.display()));
    }
}

#[tokio::test]
async fn extension_migrations_may_not_use_cedars_numbers() {
    let store = Store::connect_memory().await.unwrap();
    let (dir, migrator) = migrations(&[("0021_ee_audit.sql", "CREATE TABLE ee_audit (id INTEGER);")]).await;
    let err = store.run_extension_migrations(migrator).await.unwrap_err();
    assert!(err.to_string().contains("reserves"), "{err}");
    let _ = std::fs::remove_dir_all(dir);
}
