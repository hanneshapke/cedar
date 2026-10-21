//! Cedar for macOS and Windows: a menu bar (tray) app that runs the cedar agent server and its chat
//! UI from the binaries bundled next to it, with Docker Desktop (or OrbStack, Colima, Rancher
//! Desktop) for the sandboxes and a model server of the user's choosing. See README.md in this
//! directory.

// A GUI program on Windows: no console window next to it (debug builds keep one for the logs).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod commands;
mod config;
mod connect;
mod docker;
mod models;
mod notify;
mod paths;
mod proc;
mod supervisor;
mod tailscale;
mod tray;
mod window;

use app::AppState;
use config::Config;
use paths::Paths;
use tauri::Manager;

fn main() {
    let builder = tauri::Builder::default();
    // On a Mac, LaunchServices keeps it to one instance; on Windows a second start shows the first.
    #[cfg(windows)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _, _| {
        if let Err(e) = window::show(app) {
            eprintln!("showing the window: {e:#}");
        }
    }));
    builder
        .setup(|app| {
            // A menu bar app until its window is shown: window::present switches to a regular
            // app (Dock icon, Command-Tab) and closing the window switches back.
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
            window::create(&handle, &cfg)?;
            notify::start(handle.clone());
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
            commands::set_share,
            commands::copy_share_url,
            commands::share_qr,
        ])
        .build(tauri::generate_context!())
        .expect("Cedar could not start")
        .run(|app, event| match event {
            // Closing the window hides it; the app goes back to being a menu bar (tray) item.
            tauri::RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } => {
                api.prevent_close();
                if let Some(win) = app.get_webview_window(&label) {
                    let _ = win.hide();
                }
                #[cfg(target_os = "macos")]
                let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            }
            // An exit request without a code comes from the system: Command-Q while the window
            // is up, or the last window closing. The window is only ever hidden, never closed,
            // so this is a quit, and the Exit handler below stops the servers.
            tauri::RunEvent::ExitRequested { .. } => {}
            // Whatever ends the app, take the servers down with it.
            tauri::RunEvent::Exit => {
                // Stopping can take a while (cedar lets active runs record their state), and the
                // main thread is blocked meanwhile; don't leave a frozen window up for it.
                for win in app.webview_windows().values() {
                    let _ = win.hide();
                }
                let state = app.state::<AppState>();
                tauri::async_runtime::block_on(async {
                    state.supervisor.stop().await;
                    state.stop_share().await;
                });
            }
            _ => {}
        });
}
