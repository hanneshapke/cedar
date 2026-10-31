//! cedar-contrib: the ingest for contributed conversations (`docs/design/contrib-ingest-cloud-run.md`).
//!
//! Four routes, nothing else: `POST /v1/contributions` takes a bundle through the request path's
//! checks (§4 of the design) and stores it in quarantine with a receipt; `GET` and `DELETE
//! /v1/contributions/{id}` read its status and withdraw it, with the receipt's token; `GET
//! /v1/policy` says what this ingest takes. The slow checks (the screener) and releases are
//! separate jobs over the same quarantine bucket, not part of this service.
//!
//! The service keeps no request logs and never stores an address: the only per-request line it
//! writes is the receipt id and what became of it.

pub mod api;
pub mod config;
pub mod gcs;
pub mod ratelimit;
pub mod store;

pub use api::{router, AppState};
