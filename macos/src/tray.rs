//! The menu bar item. The app has no Dock icon, so this is how it is reached.

use crate::app::{self, AppState};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

const OPEN: &str = "open";
const BROWSER: &str = "browser";
const COPY_TOKEN: &str = "copy_token";
const IPHONE: &str = "iphone";
const SETUP: &str = "setup";
const RESTART: &str = "restart";
const LOGS: &str = "logs";
const QUIT: &str = "quit";

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let item = |id: &str, text: &str| MenuItem::with_id(app, id, text, true, None::<&str>);
    let menu = Menu::with_items(
        app,
        &[
            &item(OPEN, "Open Cedar")?,
            &item(BROWSER, "Open in Browser")?,
            &item(COPY_TOKEN, "Copy Access Token")?,
            &item(IPHONE, "iPhone & iPad Access…")?,
            &PredefinedMenuItem::separator(app)?,
            &item(SETUP, "Model Server & Setup…")?,
            &item(RESTART, "Restart Services")?,
            &item(LOGS, "Open Logs Folder")?,
            &PredefinedMenuItem::separator(app)?,
            &item(QUIT, "Quit Cedar")?,
        ],
    )?;
    let icon = tauri::image::Image::from_bytes(include_bytes!("../tray-icon.png"))?;
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(true)
        .tooltip("Cedar")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| handle(app, event.id().as_ref()))
        .build(app)?;
    Ok(())
}

fn handle(app: &AppHandle, id: &str) {
    let state = app.state::<AppState>();
    let result = match id {
        OPEN => crate::window::show(app),
        BROWSER => open::that(state.config().chat_url()).map_err(Into::into),
        COPY_TOKEN => copy_to_clipboard(&state.config().secrets.chat_access_token),
        SETUP | IPHONE => crate::window::show_setup(app),
        LOGS => open::that(&state.paths.logs).map_err(Into::into),
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

/// `pbcopy`, which every Mac has; the token is short-lived in the clipboard either way.
pub fn copy_to_clipboard(text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    let mut child = std::process::Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    child.wait()?;
    Ok(())
}
