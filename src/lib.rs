//! cedar: a backend service that runs LLM agents in sandboxed containers.
//!
//! The binary (`src/main.rs`) wires these modules together; they are a library so integration
//! tests can substitute fakes for the LLM, the sandbox, and the browser.

pub mod agent;
pub mod api;
pub mod approvals;
pub mod attachments;
pub mod auth;
pub mod browser;
pub mod compact;
pub mod config;
pub mod error;
pub mod events;
pub mod integrations;
pub mod llm;
pub mod manager;
pub mod mcp;
pub mod models;
pub mod plan;
pub mod sandbox;
pub mod secrets;
pub mod sources;
pub mod sse;
pub mod store;
pub mod tools;
pub mod waits;
