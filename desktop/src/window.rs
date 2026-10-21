//! The one window: the setup page the app serves itself, or the chat served by cedar-chat.

use crate::app::AppState;
use crate::config::Config;
use crate::supervisor::ServiceState;
use anyhow::{anyhow, Context, Result};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use url::Url;

pub const MAIN: &str = "main";

pub fn create(app: &AppHandle, cfg: &Config) -> Result<WebviewWindow> {
    let script = token_script(
        &cfg.chat_url(),
        &cfg.secrets.chat_access_token,
        &cfg.secrets.chat_admin_token,
    );
    let win = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("index.html".into()))
        .title("Cedar")
        .inner_size(1120.0, 780.0)
        .min_inner_size(720.0, 520.0)
        .initialization_script(script)
        .build()
        .context("creating the window")?;
    Ok(win)
}

/// Runs before every page the window loads. On the chat's own origin it hands the page the access
/// token the server expects (so there is no token prompt) and the admin token its Settings page
/// sends with changes to cedar's models and integrations, and tells it that it runs in the app
/// (whose web view can't take browser push; it offers Mac notifications instead); anywhere else
/// it does nothing.
fn token_script(chat_url: &str, token: &str, admin_token: &str) -> String {
    let origin = serde_json::to_string(chat_url).unwrap_or_default();
    let token = serde_json::to_string(token).unwrap_or_default();
    let admin = serde_json::to_string(admin_token).unwrap_or_default();
    format!(
        "(function () {{\n  try {{\n    if (window.location.origin === {origin}) {{\n      \
         window.localStorage.setItem('cedar-chat-token', {token});\n      \
         window.sessionStorage.setItem('cedar-chat-admin', {admin});\n      \
         window.__CEDAR_MAC_APP__ = true;\n    }}\n  }} catch (e) {{}}\n}})();"
    )
}

fn main_window(app: &AppHandle) -> Result<WebviewWindow> {
    app.get_webview_window(MAIN)
        .ok_or_else(|| anyhow!("the main window is gone"))
}

/// Shows the window and brings it forward. While the window is up the app is a regular one, with
/// a Dock icon and a Command-Tab entry; closing it (see main.rs) makes it a menu bar app again.
fn present(win: &WebviewWindow) -> Result<()> {
    #[cfg(target_os = "macos")]
    win.app_handle()
        .set_activation_policy(tauri::ActivationPolicy::Regular)
        .context("switching to a regular app")?;
    win.show()?;
    win.unminimize()?;
    win.set_focus()?;
    Ok(())
}

/// Loads the chat (unless it is already open) and brings the window forward.
pub fn show_chat(app: &AppHandle) -> Result<()> {
    let win = main_window(app)?;
    let chat = app.state::<AppState>().config().chat_url();
    let on_chat = win
        .url()
        .map(|u| u.origin().ascii_serialization() == chat)
        .unwrap_or(false);
    if !on_chat {
        win.navigate(Url::parse(&chat)?)?;
    }
    present(&win)
}

/// Opens a conversation in the chat (`None`: just the chat) and brings the window forward. On
/// the chat already, only the hash changes, so the page keeps its state.
pub fn show_conversation(app: &AppHandle, conversation: Option<&str>) -> Result<()> {
    let Some(id) = conversation else {
        return show_chat(app);
    };
    let win = main_window(app)?;
    let chat = app.state::<AppState>().config().chat_url();
    let hash = format!("#/c/{id}");
    let on_chat = win
        .url()
        .map(|u| u.origin().ascii_serialization() == chat)
        .unwrap_or(false);
    if on_chat {
        let hash = serde_json::to_string(&hash)?;
        win.eval(format!("window.location.hash = {hash};"))?;
    } else {
        win.navigate(Url::parse(&format!("{chat}/{hash}"))?)?;
    }
    present(&win)
}

/// Loads the setup page (unless it is already open) and brings the window forward.
pub fn show_setup(app: &AppHandle) -> Result<()> {
    let win = main_window(app)?;
    let state = app.state::<AppState>();
    if let Some(setup) = state.setup_url() {
        let on_setup = win.url().map(|u| u == setup).unwrap_or(false);
        if !on_setup {
            win.navigate(setup)?;
        }
    }
    present(&win)
}

/// What "Open Cedar" shows: the chat while it runs, the setup page otherwise.
pub fn show(app: &AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    if state.supervisor.state() == ServiceState::Running {
        show_chat(app)
    } else {
        show_setup(app)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_token_script_only_touches_the_chat_origin() {
        let s = token_script("http://127.0.0.1:13000", "abc\"def", "adm");
        assert!(s.contains(r#"window.location.origin === "http://127.0.0.1:13000""#));
        assert!(s.contains(r#"localStorage.setItem('cedar-chat-token', "abc\"def")"#));
        assert!(s.contains(r#"sessionStorage.setItem('cedar-chat-admin', "adm")"#));
        assert!(s.contains("window.__CEDAR_MAC_APP__ = true;"));
    }
}
