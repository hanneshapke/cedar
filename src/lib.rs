//! cedar: a backend service that runs LLM agents in sandboxes (OpenShell, on Docker).
//!
//! [`server::serve`] wires these modules together and runs the HTTP server; `src/main.rs` only
//! sets up logging and calls it. They are a library so integration tests can substitute fakes for
//! the LLM, the sandbox, and the browser, and so a downstream build can add its own routes,
//! middleware, migrations and background tasks through [`server::Extensions`].

pub mod actions;
pub mod agent;
pub mod api;
pub mod approvals;
pub mod attachments;
pub mod auth;
pub mod browser;
pub mod compact;
pub mod computer;
pub mod config;
pub mod context;
pub mod error;
pub mod events;
pub mod images;
pub mod integrations;
pub mod llm;
pub mod locale;
pub mod manager;
pub mod marketplace;
pub mod memory;
pub mod records;
pub mod routing;
pub mod mcp;
pub mod models;
#[cfg(feature = "native-computer")]
pub mod native;
pub mod openshell;
pub mod plan;
pub mod policy;
pub mod private_inference;
pub mod polls;
pub mod sandbox;
pub mod secrets;
pub mod server;
pub mod sources;
pub mod sse;
pub mod store;
pub mod tools;
pub mod timezone;
pub mod trace;
pub mod waits;
pub mod webhooks;
