//! cedar-chat: a Claude.ai-style web chat in front of a cedar server.
//!
//! The browser talks only to this service. It keeps the cedar API key server-side, stores
//! conversations, turns each user message into a cedar run (carrying earlier exchanges as
//! context), and relays the run's event stream to the page.

pub mod api;
pub mod cedar;
pub mod channels;
pub mod config;
pub mod error;
pub mod i18n;
pub mod notify;
pub mod pairing;
pub mod prompt;
pub mod schedule;
pub mod store;
pub mod update;
