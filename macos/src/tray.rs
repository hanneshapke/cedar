//! The menu bar item (on Windows, the notification area icon). The app has no Dock icon or
//! taskbar button while its window is closed, so this is how it is reached.

use crate::app::{self, AppState};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

const OPEN: &str = "open";
const BROWSER: &str = "browser";
const COPY_TOKEN: &str = "copy_token";
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
            &PredefinedMenuItem::separator(app)?,
            &item(SETUP, "Model Server & Setup…")?,
            &item(RESTART, "Restart Services")?,
            &item(LOGS, "Open Logs Folder")?,
            &PredefinedMenuItem::separator(app)?,
            &item(QUIT, "Quit Cedar")?,
        ],
    )?;
    // macOS tints the black template image to the menu bar; the Windows notification area may be
    // light or dark, so it gets the app icon in colour.
    #[cfg(target_os = "macos")]
    let icon = tauri::image::Image::from_bytes(include_bytes!("../tray-icon.png"))?;
    #[cfg(not(target_os = "macos"))]
    let icon = tauri::image::Image::from_bytes(include_bytes!("../tray-icon-color.png"))?;
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
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
        SETUP => crate::window::show_setup(app),
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

/// `pbcopy`, which every Mac has, or `clip.exe`, which every Windows has (the token is plain
/// ASCII, so its code page doesn't matter); the token is short-lived in the clipboard either way.
pub fn copy_to_clipboard(text: &str) -> anyhow::Result<()> {
    use std::io::Write;
    #[cfg(not(windows))]
    let mut command = std::process::Command::new("pbcopy");
    #[cfg(windows)]
    let mut command = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut c = std::process::Command::new("clip.exe");
        c.creation_flags(CREATE_NO_WINDOW);
        c
    };
    let mut child = command.stdin(std::process::Stdio::piped()).spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    child.wait()?;
    Ok(())
}
