//! Pairing the Cedar iOS app with a chat server, shared by cedar-chat (its Settings page) and the
//! desktop app (its setup page): the link and QR code the app scans ([`connect`]), and what
//! `tailscale status --json` says about serving the chat over HTTPS on the tailnet ([`tailscale`]).

pub mod connect;
pub mod tailscale;
