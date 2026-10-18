//! Notification Center (on Windows, toasts): cedar-chat's `desktop` notification method. While
//! the servers run, the app follows `GET /api/notifications/desktop/events` on the chat and shows each message there;
//! a click opens the conversation it is about. Which responses are worth a message (the scope,
//! a scheduled agent's own method, not if it was looked at meanwhile) is cedar-chat's decision.

use crate::app::AppState;
use crate::supervisor::ServiceState;
use crate::window;
use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// How often to look whether the servers are up, while they aren't.
const POLL: Duration = Duration::from_secs(2);

/// One message on the stream, as cedar-chat's `channels::Message` sends it.
#[derive(Deserialize, Debug, PartialEq)]
struct Message {
    title: String,
    body: String,
    /// The conversation's address in the chat (`…/#/c/<id>`).
    url: String,
}

/// Follow the chat's stream for as long as the app runs, reconnecting when it ends or fails.
pub fn start(app: AppHandle) {
    #[cfg(target_os = "macos")]
    {
        // Messages are shown as this app's own (in a bundle it is anyway; from `cargo run` it
        // only works if Cedar is installed).
        let id = app.config().identifier.clone();
        if let Err(e) = mac_notification_sys::set_application(&id) {
            eprintln!("notifications as {id}: {e}");
        }
        present_while_frontmost();
    }
    tauri::async_runtime::spawn(async move {
        let http = reqwest::Client::new();
        let mut failures: u32 = 0;
        loop {
            if app.state::<AppState>().supervisor.state() != ServiceState::Running {
                failures = 0;
                tokio::time::sleep(POLL).await;
                continue;
            }
            match follow(&app, &http).await {
                Ok(()) => failures = 0,
                Err(e) => {
                    failures += 1;
                    if failures == 1 || failures.is_multiple_of(10) {
                        eprintln!("following the chat's notifications: {e:#}");
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(1 << failures.min(5))).await;
        }
    });
}

/// macOS hides a notification's banner while the app that sent it is frontmost, unless the
/// center's delegate says otherwise, and mac-notification-sys's delegate doesn't. Cedar is
/// frontmost whenever its window has focus, e.g. when "Send a test" is clicked in the settings,
/// so the delegate's class is given `userNotificationCenter:shouldPresentNotification:` → YES.
#[cfg(target_os = "macos")]
fn present_while_frontmost() {
    use objc2::encode::Encode;
    use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
    use std::ffi::CString;

    extern "C-unwind" fn should_present(
        _: &AnyObject,
        _: Sel,
        _: &AnyObject,
        _: &AnyObject,
    ) -> Bool {
        Bool::YES
    }

    let Some(class) = AnyClass::get(c"NotificationCenterDelegate") else {
        eprintln!("notifications: mac-notification-sys's delegate class is missing");
        return;
    };
    let sel = Sel::register(c"userNotificationCenter:shouldPresentNotification:");
    let types = CString::new(format!("{}@:@@", Bool::ENCODING)).expect("no NUL in an encoding");
    // SAFETY: the function matches the selector's signature and `types` describes it. If the
    // class already has the method (a later crate version), nothing is added.
    unsafe {
        let imp: Imp = std::mem::transmute(
            should_present as extern "C-unwind" fn(&AnyObject, Sel, &AnyObject, &AnyObject) -> Bool,
        );
        objc2::ffi::class_addMethod(
            class as *const AnyClass as *mut AnyClass,
            sel,
            imp,
            types.as_ptr(),
        );
    }
}

/// One connection: show every message until the stream ends.
async fn follow(app: &AppHandle, http: &reqwest::Client) -> Result<()> {
    let cfg = app.state::<AppState>().config();
    let resp = http
        .get(format!(
            "{}/api/notifications/desktop/events",
            cfg.chat_url()
        ))
        .bearer_auth(&cfg.secrets.chat_access_token)
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(anyhow!("the chat answered {}", resp.status()));
    }
    let mut body = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = body.next().await {
        buf.extend_from_slice(&chunk?);
        while let Some(end) = buf.windows(2).position(|w| w == b"\n\n") {
            let block: Vec<u8> = buf.drain(..end + 2).collect();
            if let Some(message) = parse_sse(&block).and_then(|v| serde_json::from_value(v).ok()) {
                show(app, message);
            }
        }
    }
    Ok(())
}

/// The JSON of one SSE block's `data:` lines; `None` for a keep-alive comment or malformed data.
fn parse_sse(block: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(block).ok()?;
    let data: Vec<&str> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(|rest| rest.strip_prefix(' ').unwrap_or(rest))
        .collect();
    if data.is_empty() {
        return None;
    }
    serde_json::from_str(&data.join("\n")).ok()
}

/// The conversation a message's URL points at (`#/c/<uuid>`), if any.
fn conversation_of(url: &str) -> Option<String> {
    let (_, fragment) = url.split_once('#')?;
    let id = fragment.strip_prefix("/c/")?;
    uuid_like(id).then(|| id.to_string())
}

fn uuid_like(s: &str) -> bool {
    s.len() == 36
        && s.chars().enumerate().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

/// Post the message to Notification Center. Waiting for a click blocks, so each waits on a
/// thread of its own, which ends when the notification is clicked or dismissed. On Windows the
/// toast calls back on a click instead, so its thread ends once the toast is up.
fn show(app: &AppHandle, message: Message) {
    let conversation = conversation_of(&message.url);
    let app = app.clone();
    std::thread::spawn(move || {
        #[cfg(target_os = "macos")]
        {
            use mac_notification_sys::{Notification, NotificationResponse};
            let response = Notification::new()
                .title(&message.title)
                .message(&message.body)
                .default_sound()
                .wait_for_click(true)
                .send();
            match response {
                Ok(NotificationResponse::Click) => {
                    let opened = app.clone();
                    let _ = app.run_on_main_thread(move || {
                        if let Err(e) = window::show_conversation(&opened, conversation.as_deref())
                        {
                            eprintln!("opening the conversation: {e:#}");
                        }
                    });
                }
                Ok(_) => {}
                Err(e) => eprintln!("showing a notification: {e}"),
            }
        }
        #[cfg(windows)]
        {
            use tauri_winrt_notification::Toast;
            let opened = app.clone();
            let shown = Toast::new(&toast_app_id(&app))
                .title(&message.title)
                .text1(&message.body)
                .on_activated(move |_| {
                    let app = opened.clone();
                    let conversation = conversation.clone();
                    let _ = opened.run_on_main_thread(move || {
                        if let Err(e) = window::show_conversation(&app, conversation.as_deref()) {
                            eprintln!("opening the conversation: {e:#}");
                        }
                    });
                    Ok(())
                })
                .show();
            if let Err(e) = shown {
                eprintln!("showing a notification: {e}");
            }
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        let _ = (app, message, conversation);
    });
}

/// The AppUserModelID a toast is sent as. The installer gives Cedar's Start menu shortcut the
/// bundle identifier as its id, which is what makes Windows show the toast as Cedar's; a build
/// run from `target/` has no shortcut, so it borrows PowerShell's, as tauri-plugin-notification
/// does.
#[cfg(windows)]
fn toast_app_id(app: &AppHandle) -> String {
    let from_target = std::env::current_exe()
        .map(|exe| exe.ancestors().any(|a| a.file_name() == Some("target".as_ref())))
        .unwrap_or(false);
    if from_target {
        tauri_winrt_notification::Toast::POWERSHELL_APP_ID.to_string()
    } else {
        app.config().identifier.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_read_off_the_stream() {
        assert_eq!(parse_sse(b": keep-alive\n\n"), None);
        let block = b"event: notification\ndata: {\"type\":\"notification\",\"title\":\"T\",\"body\":\"B\",\"url\":\"http://x/#/c/1\",\"tag\":\"t\"}\n\n";
        let message: Message = serde_json::from_value(parse_sse(block).unwrap()).unwrap();
        assert_eq!(
            message,
            Message {
                title: "T".into(),
                body: "B".into(),
                url: "http://x/#/c/1".into()
            }
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn banners_show_while_cedar_is_frontmost() {
        use objc2::runtime::{AnyClass, Sel};
        present_while_frontmost();
        let class = AnyClass::get(c"NotificationCenterDelegate").unwrap();
        let sel = Sel::register(c"userNotificationCenter:shouldPresentNotification:");
        assert!(class.instance_method(sel).is_some());
    }

    #[test]
    fn only_a_conversation_id_is_taken_from_the_url() {
        let id = "0192f0c4-1b2c-7d3e-8f40-123456789abc";
        assert_eq!(
            conversation_of(&format!("http://127.0.0.1:13000/#/c/{id}")).as_deref(),
            Some(id)
        );
        assert_eq!(conversation_of("http://127.0.0.1:13000/"), None);
        assert_eq!(conversation_of("http://x/#/c/'); alert(1); ('"), None);
    }
}
