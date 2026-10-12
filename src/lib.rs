//! cedar: a backend service that runs Claude agents in sandboxed containers.
//!
//! The binary (`src/main.rs`) wires these modules together; they are a library so integration
//! tests can substitute fakes for the LLM and the sandbox.

pub mod agent;
pub mod api;
pub mod auth;
pub mod config;
pub mod error;
pub mod events;
pub mod llm;
pub mod manager;
pub mod models;
pub mod sandbox;
pub mod sse;
pub mod store;
pub mod tools;
