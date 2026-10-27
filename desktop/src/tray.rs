//! The menu bar item (a tray icon on Windows). The app has no Dock icon or taskbar button while its
//! window is closed, so this is how it is reached. On a Mac a click opens the menu; on Windows a
//! left click opens the window and a right click the menu, as tray icons do there.

use crate::app::{self, AppState};
use crate::i18n::{t, tf};
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

/// The menu's items with the text key of each, to relabel when the language changes, and the
/// update item, which [`show_update`] relabels as well.
struct TrayItems {
    labeled: Vec<(&'static str, MenuItem<tauri::Wry>)>,
    update: MenuItem<tauri::Wry>,
}

pub fn build(app: &AppHandle) -> tauri::Result<()> {
    let mut labeled = Vec::new();
    let mut item = |id: &str, key: &'static str| -> tauri::Result<MenuItem<tauri::Wry>> {
        let it = MenuItem::with_id(app, id, t(key), true, None::<&str>)?;
        labeled.push((key, it.clone()));
        Ok(it)
    };
    let open = item(OPEN, "tray.open")?;
    let browser = item(BROWSER, "tray.browser")?;
    let copy_token = item(COPY_TOKEN, "tray.copy_token")?;
    let iphone = item(IPHONE, "tray.phone")?;
    let setup = item(SETUP, "tray.setup")?;
    let restart = item(RESTART, "tray.restart")?;
    let logs = item(LOGS, "tray.logs")?;
    let quit = item(QUIT, "tray.quit")?;
    let update = MenuItem::with_id(app, UPDATE, t("tray.check_updates"), true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &browser,
            &copy_token,
            &iphone,
            &PredefinedMenuItem::separator(app)?,
            &setup,
            &restart,
            &logs,
            &update,
            &PredefinedMenuItem::separator(app)?,
            &quit,
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
    app.manage(TrayItems { labeled, update });
    Ok(())
}

/// Puts the menu into the language just picked (see `commands::set_locale`).
pub fn relabel(app: &AppHandle) {
    let Some(items) = app.try_state::<TrayItems>() else {
        return;
    };
    for (key, item) in &items.labeled {
        let _ = item.set_text(t(key));
    }
    show_update(app, &app.state::<AppState>().update());
}

/// "Update to Cedar 0.2.0…" once a newer release is known, so it can be found with the window
/// closed; the window's banner then asks before installing.
pub fn show_update(app: &AppHandle, s: &crate::update::UpdateStatus) {
    let Some(items) = app.try_state::<TrayItems>() else {
        return;
    };
    let (text, enabled) = match (s.state, &s.version) {
        ("downloading" | "installing", _) => (t("tray.installing_update"), false),
        ("checking", _) => (t("tray.checking_updates"), false),
        (_, Some(v)) => (tf("tray.update_to", &[("version", v)]), true),
        _ => (t("tray.check_updates"), s.enabled),
    };
    let _ = items.update.set_text(text);
    let _ = items.update.set_enabled(enabled);
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
