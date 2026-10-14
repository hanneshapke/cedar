//! Cedar for macOS: a menu bar app that runs the cedar agent server and its chat UI from the
//! binaries bundled next to it, with Docker Desktop (or OrbStack, Colima, Rancher Desktop) for
//! the sandboxes and a model server of the user's choosing. See README.md in this directory.

mod app;
mod commands;
mod config;
mod docker;
mod models;
mod paths;
mod supervisor;
mod tray;
mod window;

use app::AppState;
use config::Config;
use paths::Paths;
use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            // No Dock icon: the app lives in the menu bar and its window opens from there.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let handle = app.handle().clone();
            let paths = Paths::resolve(&handle)?;
            let cfg = Config::load_or_create(&paths.config())?;
            let exe_dir = std::env::current_exe()?
                .parent()
                .ok_or("the executable has no parent directory")?
                .to_path_buf();
            app.manage(AppState::new(paths, cfg.clone(), exe_dir));

            tray::build(&handle)?;
            let win = window::create(&handle, &cfg)?;
            if let Ok(url) = win.url() {
                app.state::<AppState>().set_setup_url(url);
            }
            tauri::async_runtime::spawn(app::bootstrap(handle));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::recheck,
            commands::list_models,
            commands::save_model_server,
            commands::set_browser,
            commands::open_chat,
            commands::open_in_browser,
            commands::open_logs,
            commands::copy_token,
        ])
        .build(tauri::generate_context!())
        .expect("Cedar could not start")
        .run(|app, event| match event {
            // Closing the window hides it; the app stays in the menu bar.
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } => {
                api.prevent_close();
                if let Some(win) = app.get_webview_window(&label) {
                    let _ = win.hide();
                }
            }
            // With no windows left Tauri would exit; only Quit (which passes a code) may.
            tauri::RunEvent::ExitRequested { api, code, .. } => {
                if code.is_none() {
                    api.prevent_exit();
                }
            }
            // Whatever ends the app, take the servers down with it.
            tauri::RunEvent::Exit => {
                let state = app.state::<AppState>();
                tauri::async_runtime::block_on(state.supervisor.stop());
            }
            _ => {}
        });
}
