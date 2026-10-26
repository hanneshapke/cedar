//! The menu bar item (a tray icon on Windows). The app has no Dock icon or taskbar button while its
//! window is closed, so this is how it is reached. On a Mac a click opens the menu; on Windows a
//! left click opens the window and a right click the menu, as tray icons do there.

use crate::app::{self, AppState};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

const OPEN: &str = "open";
const BROWSER: &str = "browser";
const COPY_TOKEN: &str = "copy_token";
const IPHONE: &str = "iphone";
const SETUP: &str = "setup";
const RESTART: &str = "restart";
const LOGS: &str = "logs";
const UPDATE: &str = "update";
const QUIT: &str = "quit";

/// The menu's update item, which [`show_update`] relabels.
struct UpdateItem(MenuItem<tauri::Wry>);

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let item = |id: &str, text: &str| MenuItem::with_id(app, id, text, true, None::<&str>);
    let update = item(UPDATE, "Check for Updates…")?;
    let menu = Menu::with_items(
        app,
        &[
            &item(OPEN, "Open Cedar")?,
            &item(BROWSER, "Open in Browser")?,
            &item(COPY_TOKEN, "Copy Access Token")?,
            &item(IPHONE, "Phone Access…")?,
            &PredefinedMenuItem::separator(app)?,
            &item(SETUP, "Model Server & Setup…")?,
            &item(RESTART, "Restart Services")?,
            &item(LOGS, "Open Logs Folder")?,
            &update,
            &PredefinedMenuItem::separator(app)?,
            &item(QUIT, "Quit Cedar")?,
        ],
    )?;
    // A template image on a Mac, tinted to the menu bar; Windows draws icons as they are, and a
    // black mark would vanish on a dark taskbar, so there it is the app icon in colour.
    #[cfg(target_os = "macos")]
    let icon = tauri::image::Image::from_bytes(include_bytes!("../tray-icon.png"))?;
    #[cfg(not(target_os = "macos"))]
    let icon = tauri::image::Image::from_bytes(include_bytes!("../tray-icon-color.png"))?;
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("Cedar")
        .menu(&menu)
        .show_menu_on_left_click(cfg!(target_os = "macos"))
        .on_menu_event(|app, event| handle(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if cfg!(target_os = "macos") {
                return;
            }
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                if let Err(e) = crate::window::show(tray.app_handle()) {
                    eprintln!("showing the window: {e:#}");
                }
            }
        })
        .build(app)?;
    app.manage(UpdateItem(update));
    Ok(())
}

/// "Update to Cedar 0.2.0…" once a newer release is known, so it can be found with the window
/// closed; the window's banner then asks before installing.
pub fn show_update(app: &AppHandle, s: &crate::update::UpdateStatus) {
    let Some(item) = app.try_state::<UpdateItem>() else {
        return;
    };
    let (text, enabled) = match (s.state, &s.version) {
        ("downloading" | "installing", _) => ("Installing the Update…".to_string(), false),
        ("checking", _) => ("Checking for Updates…".to_string(), false),
        (_, Some(v)) => (format!("Update to Cedar {v}…"), true),
        _ => ("Check for Updates…".to_string(), s.enabled),
    };
    let _ = item.0.set_text(text);
    let _ = item.0.set_enabled(enabled);
}

fn handle(app: &AppHandle, id: &str) {
    let state = app.state::<AppState>();
    let result = match id {
        OPEN => crate::window::show(app),
        BROWSER => open::that(state.config().chat_url()).map_err(Into::into),
        COPY_TOKEN => copy_to_clipboard(&state.config().secrets.chat_access_token),
        SETUP | IPHONE => crate::window::show_setup(app),
        LOGS => open::that(&state.paths.logs).map_err(Into::into),
        UPDATE => {
            if state.update().version.is_none() {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = crate::update::check(&app).await {
                        eprintln!("checking for updates: {e:#}");
                    }
                });
            }
            // The banner (on the chat or the setup page) says what was found and asks.
            crate::window::show(app)
        }
        RESTART => {
            tauri::async_runtime::spawn(app::restart(app.clone()));
            Ok(())
        }
        QUIT => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                app.state::<AppState>().supervisor.stop().await;
                app.exit(0);
            });
            Ok(())
        }
        _ => Ok(()),
    };
    if let Err(e) = result {
        eprintln!("menu action {id}: {e:#}");
    }
}

/// `pbcopy`, which every Mac has, or `clip.exe` on Windows; the token is short-lived in the
/// clipboard either way. Both are ASCII here (a hex token, a URL), which `clip` takes as it is.
pub fn copy_to_clipboard(text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let program = if cfg!(windows) { "clip" } else { "pbcopy" };
    let mut child = crate::proc::std_command(program)
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    child.wait()?;
    Ok(())
}
