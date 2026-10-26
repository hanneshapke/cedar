//! What the setup page may ask the app to do. Nothing here hands the page a server key.

use crate::app::{self, AppState, Status};
use crate::models;
use tauri::{AppHandle, Manager, State};

fn fail(e: anyhow::Error) -> String {
    format!("{e:#}")
}

#[tauri::command]
pub fn get_status(state: State<'_, AppState>) -> Status {
    state.status()
}

/// Runs the launch sequence again, after Docker was started or a pull failed.
#[tauri::command]
pub fn recheck(app: AppHandle) {
    tauri::async_runtime::spawn(app::bootstrap(app));
}

/// Sets Docker up: WSL 2 and Docker Desktop on Windows, Docker Desktop on a Mac. The page follows
/// along in `docker_setup`.
#[tauri::command]
pub fn install_docker(app: AppHandle) -> Result<(), String> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        if !app.state::<AppState>().begin_docker_setup() {
            return Err("Docker is already being set up".into());
        }
        #[cfg(windows)]
        tauri::async_runtime::spawn(crate::winsetup::install(app));
        #[cfg(target_os = "macos")]
        tauri::async_runtime::spawn(crate::macsetup::install(app));
        Ok(())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = app;
        Err("Cedar sets Docker up only on Windows and macOS".into())
    }
}

/// Restarts the PC or signs out to finish the Docker setup, and only when it asks for that.
#[tauri::command]
pub fn finish_docker_setup(state: State<'_, AppState>) -> Result<(), String> {
    let sign_out = match state.docker_setup() {
        app::SetupStatus::NeedsSignOut => true,
        app::SetupStatus::NeedsRestart => false,
        _ => return Err("the Docker setup doesn't need a restart".into()),
    };
    #[cfg(windows)]
    return crate::winsetup::restart(sign_out).map_err(fail);
    #[cfg(not(windows))]
    {
        let _ = sign_out;
        Err("only on Windows".into())
    }
}

#[tauri::command]
pub async fn list_models(base_url: String, api_key: String) -> Result<Vec<String>, String> {
    models::list_models(&base_url, &api_key).await.map_err(fail)
}

/// Saves the model server and registers it with cedar (starting the servers first if needed).
#[tauri::command]
pub fn save_model_server(
    app: AppHandle,
    base_url: String,
    api_key: String,
    model: String,
) -> Result<(), String> {
    let base_url = models::normalize_base_url(&base_url).map_err(fail)?;
    let model = model.trim().to_string();
    if model.is_empty() {
        return Err("pick a model".into());
    }
    app.state::<AppState>()
        .update_config(|c| {
            c.llm_base_url = base_url;
            c.llm_api_key = api_key.trim().to_string();
            c.llm_model = model;
        })
        .map_err(fail)?;
    tauri::async_runtime::spawn(app::apply_model_server(app));
    Ok(())
}

/// Turns the browser tool on or off; on pulls its image and restarts cedar with `BROWSER=on`.
#[tauri::command]
pub fn set_browser(app: AppHandle, enabled: bool) -> Result<(), String> {
    app.state::<AppState>()
        .update_config(|c| c.browser = enabled)
        .map_err(fail)?;
    tauri::async_runtime::spawn(app::restart(app));
    Ok(())
}

/// Shares the chat with the phone apps (iOS, Android) over Tailscale, or stops sharing it.
#[tauri::command]
pub async fn set_share(app: AppHandle, enabled: bool) -> Result<(), String> {
    app::set_share(app, enabled).await.map_err(fail)
}

#[tauri::command]
pub fn copy_share_url(state: State<'_, AppState>) -> Result<(), String> {
    let url = state
        .share_url()
        .ok_or("the chat is not shared on the tailnet")?;
    crate::tray::copy_to_clipboard(&url).map_err(fail)
}

/// The QR code the iOS and Android apps scan to connect, as SVG markup. It holds the access token, so the
/// page shows it only when asked.
#[tauri::command]
pub fn share_qr(state: State<'_, AppState>) -> Result<String, String> {
    let url = state
        .share_url()
        .ok_or("the chat is not shared on the tailnet")?;
    let link = cedar_pairing::connect::link(&url, &state.config().secrets.chat_access_token);
    cedar_pairing::connect::qr_svg(&link).map_err(fail)
}

#[tauri::command]
pub fn open_chat(app: AppHandle) -> Result<(), String> {
    crate::window::show_chat(&app).map_err(fail)
}

#[tauri::command]
pub fn open_in_browser(state: State<'_, AppState>) -> Result<(), String> {
    open::that(state.config().chat_url()).map_err(|e| e.to_string())
}

/// Opens a link from the setup page (Get Docker Desktop, Tailscale, Docker's terms) in the default browser: the
/// web view doesn't follow `target="_blank"` links itself (on Windows, WebView2 drops them).
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    let parsed = url::Url::parse(&url).map_err(|e| e.to_string())?;
    crate::window::open_web_link(&parsed).map_err(fail)
}

#[tauri::command]
pub fn open_logs(state: State<'_, AppState>) -> Result<(), String> {
    open::that(&state.paths.logs).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn copy_token(state: State<'_, AppState>) -> Result<(), String> {
    crate::tray::copy_to_clipboard(&state.config().secrets.chat_access_token).map_err(fail)
}

/// "Check now" on the setup page.
#[tauri::command]
pub async fn check_for_updates(app: AppHandle) -> Result<(), String> {
    crate::update::check(&app).await.map_err(fail)
}

/// The banner's Update: downloads and installs the newer release, then restarts into it.
#[tauri::command]
pub fn install_update(app: AppHandle) {
    tauri::async_runtime::spawn(crate::update::install(app));
}

/// Hides the banner until a later release than this one is out.
#[tauri::command]
pub fn dismiss_update(app: AppHandle) -> Result<(), String> {
    crate::update::dismiss(&app).map_err(fail)
}

#[tauri::command]
pub fn set_check_updates(app: AppHandle, enabled: bool) -> Result<(), String> {
    crate::update::set_enabled(&app, enabled).map_err(fail)
}
