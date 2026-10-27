//! The one window: the setup page the app serves itself, or the chat served by cedar-chat.

use crate::app::AppState;
use crate::config::Config;
use crate::supervisor::ServiceState;
use anyhow::{anyhow, Context, Result};
use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use url::Url;

pub const MAIN: &str = "main";

/// Where the chat's update banner (update.js) sends the window for Update and Dismiss: never
/// loaded, the window catches it (see [`create`]). The chat page has no other way to reach the app.
const UPDATE_PATH: &str = "/__cedar-app/update/";

/// The setup page's own URL, where Tauri serves `ui/index.html`: `tauri://localhost` on a Mac,
/// `http://tauri.localhost` on Windows. Not read back from the window after it is built: on
/// Windows that is still `about:blank` until WebView2 commits its first navigation, and going
/// back there from the tray left the window blank.
fn setup_url() -> Url {
    let url = if cfg!(windows) {
        "http://tauri.localhost/index.html"
    } else {
        "tauri://localhost/index.html"
    };
    Url::parse(url).expect("the setup page's URL is valid")
}

pub fn create(app: &AppHandle, cfg: &Config) -> Result<WebviewWindow> {
    let script = token_script(
        &cfg.chat_url(),
        &cfg.secrets.chat_access_token,
        &cfg.secrets.chat_admin_token,
    );
    let chat = cfg.chat_url();
    let handle = app.clone();
    let win = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("index.html".into()))
        .title("Cedar")
        .inner_size(1120.0, 780.0)
        .min_inner_size(720.0, 520.0)
        .initialization_script(script)
        .on_new_window(|url, _| {
            if let Err(e) = open_web_link(&url) {
                eprintln!("opening {url}: {e:#}");
            }
            NewWindowResponse::Deny
        })
        .on_navigation(move |url| match update_action(url, &chat) {
            Some(action) => {
                banner_action(&handle, action);
                false
            }
            None => true,
        })
        .on_page_load(|webview, payload| {
            if payload.event() == PageLoadEvent::Finished {
                let status = webview.app_handle().state::<AppState>().update();
                push_update(webview.app_handle(), &status);
            }
        })
        .build()
        .context("creating the window")?;
    Ok(win)
}

/// Opens a link in the default browser. The window never opens a second one: a link the chat
/// opens in a new tab (`target="_blank"`, `window.open`), such as a source in an answer's
/// references, goes here instead; the web view drops such links on its own (WKWebView on a Mac
/// opens nothing). Only http(s) URLs, so a page can't have the app open a file or run a program.
pub fn open_web_link(url: &Url) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(anyhow!("not a web link: {url}"));
    }
    open::that(url.as_str()).context("opening the default browser")
}

/// Runs before every page the window loads. On the chat's own origin it hands the page the access
/// token the server expects (so there is no token prompt) and the admin token its Settings page
/// sends with changes to cedar's models and integrations, and tells it that it runs in the app
/// (whose web view can't take browser push; it offers desktop notifications instead); anywhere
/// else it does nothing.
fn token_script(chat_url: &str, token: &str, admin_token: &str) -> String {
    let origin = serde_json::to_string(chat_url).unwrap_or_default();
    let token = serde_json::to_string(token).unwrap_or_default();
    let admin = serde_json::to_string(admin_token).unwrap_or_default();
    format!(
        "(function () {{\n  try {{\n    if (window.location.origin === {origin}) {{\n      \
         window.localStorage.setItem('cedar-chat-token', {token});\n      \
         window.sessionStorage.setItem('cedar-chat-admin', {admin});\n      \
         window.__CEDAR_DESKTOP_APP__ = true;\n    }}\n  }} catch (e) {{}}\n}})();"
    )
}

#[derive(Debug, PartialEq)]
enum UpdateAction {
    Install,
    Dismiss,
}

/// The banner's Update or Dismiss, when `url` is the chat's address for it.
fn update_action(url: &Url, chat: &str) -> Option<UpdateAction> {
    if url.origin().ascii_serialization() != chat {
        return None;
    }
    match url.path().strip_prefix(UPDATE_PATH)? {
        "install" => Some(UpdateAction::Install),
        "dismiss" => Some(UpdateAction::Dismiss),
        _ => None,
    }
}

fn banner_action(app: &AppHandle, action: UpdateAction) {
    match action {
        // Only does something while a newer release is known; it is the genuine, signed one.
        UpdateAction::Install => {
            tauri::async_runtime::spawn(crate::update::install(app.clone()));
        }
        UpdateAction::Dismiss => {
            if let Err(e) = crate::update::dismiss(app) {
                eprintln!("dismissing the update: {e:#}");
            }
        }
    }
}

/// Tells the chat page's banner what the updater knows, when the window shows the chat. The
/// setup page asks for it itself (`get_status`).
pub fn push_update(app: &AppHandle, status: &crate::update::UpdateStatus) {
    let Ok(win) = main_window(app) else {
        return;
    };
    let chat = app.state::<AppState>().config().chat_url();
    let on_chat = win
        .url()
        .map(|u| u.origin().ascii_serialization() == chat)
        .unwrap_or(false);
    if !on_chat {
        return;
    }
    let json = crate::update::for_chat(status).to_string();
    // `cedarUpdate` is a top-level `const` of update.js, not a property of window.
    let _ = win.eval(format!(
        "if (typeof cedarUpdate !== 'undefined') cedarUpdate.fromApp({json});"
    ));
}

fn main_window(app: &AppHandle) -> Result<WebviewWindow> {
    app.get_webview_window(MAIN)
        .ok_or_else(|| anyhow!("the main window is gone"))
}

/// Shows the window and brings it forward. While the window is up the app is a regular one, with
/// a Dock icon and a Command-Tab entry (a taskbar button on Windows); closing it (see main.rs)
/// makes it a menu bar (tray) app again.
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
    let setup = setup_url();
    let on_setup = win.url().map(|u| u == setup).unwrap_or(false);
    if !on_setup {
        win.navigate(setup)?;
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
        assert!(s.contains("window.__CEDAR_DESKTOP_APP__ = true;"));
    }

    #[test]
    fn only_web_links_open_in_the_browser() {
        for url in [
            "file:///etc/passwd",
            "blob:http://127.0.0.1:13000/x",
            "javascript:alert(1)",
        ] {
            assert!(open_web_link(&Url::parse(url).unwrap()).is_err(), "{url}");
        }
    }

    #[test]
    fn only_the_chats_update_addresses_reach_the_app() {
        let chat = "http://127.0.0.1:13000";
        let at = |u: &str| update_action(&Url::parse(u).unwrap(), chat);
        assert_eq!(at("http://127.0.0.1:13000/__cedar-app/update/install"), Some(UpdateAction::Install));
        assert_eq!(at("http://127.0.0.1:13000/__cedar-app/update/dismiss?x=1"), Some(UpdateAction::Dismiss));
        assert_eq!(at("http://127.0.0.1:13000/__cedar-app/update/other"), None);
        assert_eq!(at("http://127.0.0.1:13000/"), None);
        assert_eq!(at("http://127.0.0.1:13001/__cedar-app/update/install"), None);
        assert_eq!(at("https://example.com/__cedar-app/update/install"), None);
    }

    #[test]
    fn the_setup_page_is_the_apps_own_index() {
        let url = setup_url();
        assert_eq!(url.path(), "/index.html");
        assert_ne!(url.scheme(), "about");
    }
}
