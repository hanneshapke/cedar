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

/// Shares the chat with the iOS app over Tailscale, or stops sharing it.
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

#[tauri::command]
pub fn open_chat(app: AppHandle) -> Result<(), String> {
    crate::window::show_chat(&app).map_err(fail)
}

#[tauri::command]
pub fn open_in_browser(state: State<'_, AppState>) -> Result<(), String> {
    open::that(state.config().chat_url()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_logs(state: State<'_, AppState>) -> Result<(), String> {
    open::that(&state.paths.logs).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn copy_token(state: State<'_, AppState>) -> Result<(), String> {
    crate::tray::copy_to_clipboard(&state.config().secrets.chat_access_token).map_err(fail)
}
