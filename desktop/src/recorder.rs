//! Recording the screen for the chat's Actions page (chat/static/actions.js), on a Mac.
//!
//! A page in a browser records the screen itself (`getDisplayMedia`). The app's window can't on a
//! Mac: WKWebView offers no screen sharing. So the app records for it, with the recorder macOS
//! ships, `/usr/sbin/screencapture -v`, run as a child process. It needs no code of its own that
//! touches the screen, and the system's Screen Recording permission covers it as part of Cedar,
//! since Cedar started it. On Windows the window (WebView2) records like a browser, so none of
//! this is used there.
//!
//! The page asks through the window (see `window::PageAction`):
//!
//!   /__cedar-app/record/start?max=600   record the entire screen, for at most that many seconds
//!   /__cedar-app/record/stop            stop and hand the recording over
//!   /__cedar-app/record/cancel          stop and throw it away
//!
//! and hears back as `cedar-recorder` events on its window, each with a `detail`:
//!
//!   {"state": "recording"}
//!   {"state": "done", "url": "cedarrec://localhost/<token>", "seconds": 42, "media_type": "video/quicktime"}
//!   {"state": "failed", "reason": "permission" | "empty" | "unavailable" | "busy", "message"?}
//!   {"state": "cancelled"}
//!
//! The page fetches `url` once: the app serves the file through its own URL scheme (see
//! [`serve`]) and deletes it. The address holds a random token, so only the page that was told it
//! can read the recording, and nothing of it is kept. The page then makes the recording small
//! and sends its key frames to cedar, as it does with one it recorded itself.

use crate::app::AppState;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};
use tokio::io::AsyncWriteExt;
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::oneshot;

/// The URL scheme recordings are handed to the page through.
pub const SCHEME: &str = "cedarrec";
/// macOS's own recorder.
const SCREENCAPTURE: &str = "/usr/sbin/screencapture";
/// The longest recording when the page names none, and the longest it may name, in seconds.
const DEFAULT_MAX_SECS: u64 = 600;
const LIMIT_MAX_SECS: u64 = 1800;
/// How long the recorder gets to finish its file after each way of telling it to stop.
const AFTER_KEY: Duration = Duration::from_secs(3);
const AFTER_SIGNAL: Duration = Duration::from_secs(15);

/// How a recording under way is to end.
#[derive(Debug, Clone, Copy, PartialEq)]
enum End {
    /// Finish the file and hand it over.
    Keep,
    /// Throw it away.
    Discard,
}

/// A recording that is finished and waits for the page to fetch it.
struct Finished {
    token: String,
    path: PathBuf,
}

#[derive(Default)]
pub struct Recorder {
    /// Ends the recording under way; `None` while nothing is recorded.
    active: Mutex<Option<oneshot::Sender<End>>>,
    finished: Mutex<Option<Finished>>,
}

impl Recorder {
    /// Whether this app records for its chat page: on a Mac, which has the recorder.
    pub fn available() -> bool {
        cfg!(target_os = "macos") && Path::new(SCREENCAPTURE).exists()
    }

    /// Forget a finished recording nobody fetched, and its file.
    fn discard_finished(&self) {
        if let Some(old) = self.finished.lock().unwrap().take() {
            let _ = std::fs::remove_file(old.path);
        }
    }

    fn end(&self, how: End) -> bool {
        match self.active.lock().unwrap().take() {
            Some(stop) => stop.send(how).is_ok(),
            None => false,
        }
    }
}

/// Whether the system lets Cedar record the screen; asking shows the system's prompt the first
/// time (and afterwards nothing: the user then allows it in System Settings).
#[cfg(target_os = "macos")]
mod access {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    pub fn granted() -> bool {
        // SAFETY: both take no arguments and only read (or ask for) this process's permission.
        unsafe { CGPreflightScreenCaptureAccess() }
    }

    pub fn request() {
        // SAFETY: as above.
        unsafe {
            CGRequestScreenCaptureAccess();
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod access {
    pub fn granted() -> bool {
        true
    }

    pub fn request() {}
}

/// Tell the chat page what became of its request.
fn push(app: &AppHandle, detail: Value) {
    let Some(win) = app.get_webview_window(crate::window::MAIN) else {
        return;
    };
    let _ = win.eval(format!(
        "window.dispatchEvent(new CustomEvent('cedar-recorder', {{ detail: {detail} }}));"
    ));
}

fn failed(app: &AppHandle, reason: &str, message: Option<String>) {
    push(app, json!({ "state": "failed", "reason": reason, "message": message }));
}

/// The arguments for a recording of the entire screen into `file`: video (`-v`), with clicks
/// shown (`-k`, so whoever watches sees what was pressed), no sounds (`-x`), and an end after
/// `max_secs` at the latest (`-V`).
fn capture_args(file: &Path, max_secs: u64) -> Vec<String> {
    vec![
        "-v".into(),
        "-k".into(),
        "-x".into(),
        "-V".into(),
        max_secs.to_string(),
        file.to_string_lossy().into_owned(),
    ]
}

fn token() -> String {
    use rand::RngCore;
    let mut buf = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    hex::encode(buf)
}

/// Start recording the entire screen. Answers the page with `recording`, or `failed` and why.
pub fn start(app: &AppHandle, max_secs: Option<u64>) {
    let state = app.state::<AppState>();
    let recorder = state.recorder();
    if !Recorder::available() {
        return failed(app, "unavailable", None);
    }
    if recorder.active.lock().unwrap().is_some() {
        return failed(app, "busy", None);
    }
    if !access::granted() {
        // The first time this shows the system's prompt; either way the page says where the
        // switch is, and that Cedar has to be started again once it is on.
        access::request();
        return failed(app, "permission", None);
    }
    recorder.discard_finished();
    let max_secs = max_secs.unwrap_or(DEFAULT_MAX_SECS).clamp(5, LIMIT_MAX_SECS);
    let token = token();
    let path = std::env::temp_dir().join(format!("cedar-recording-{token}.mov"));
    let (stop, stopped) = oneshot::channel();
    *recorder.active.lock().unwrap() = Some(stop);
    let app = app.clone();
    // This is called from the window's navigation handler, on the main thread, which is no
    // part of the async runtime. The recorder is a tokio child process, and tokio can only
    // start one from inside its runtime (it panics otherwise, which there aborts the app), so
    // everything from starting it on happens in a task.
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let recorder = state.recorder();
        let child = match spawn(Path::new(SCREENCAPTURE), &capture_args(&path, max_secs)).await {
            Ok(child) => child,
            Err(e) => {
                recorder.active.lock().unwrap().take();
                return failed(&app, "unavailable", Some(format!("{e:#}")));
            }
        };
        push(&app, json!({ "state": "recording" }));
        let started = Instant::now();
        let outcome = run(child, stopped, &path).await;
        recorder.active.lock().unwrap().take();
        match outcome {
            Outcome::Recorded => {
                *recorder.finished.lock().unwrap() = Some(Finished { token: token.clone(), path });
                push(
                    &app,
                    json!({ "state": "done", "url": format!("{SCHEME}://localhost/{token}"),
                            "seconds": started.elapsed().as_secs(), "media_type": "video/quicktime" }),
                );
            }
            Outcome::Discarded => push(&app, json!({ "state": "cancelled" })),
            // Nothing was written although the recorder ran: the system kept it from the screen.
            Outcome::Empty => failed(&app, "empty", None),
        }
    });
}

/// Stop the recording under way and hand it to the page (`done` follows).
pub fn stop(app: &AppHandle) {
    app.state::<AppState>().recorder().end(End::Keep);
}

/// Stop the recording under way, or forget the finished one, without handing anything over.
pub fn cancel(app: &AppHandle) {
    let state = app.state::<AppState>();
    let recorder = state.recorder();
    if !recorder.end(End::Discard) {
        recorder.discard_finished();
    }
}

/// Start the recorder. Async although nothing in it waits: a tokio child process can only be
/// started inside the runtime, and this way it can't be called from anywhere else.
async fn spawn(program: &Path, args: &[String]) -> Result<(Child, ChildStdin)> {
    let mut child = Command::new(program)
        .args(args)
        // The recorder stops at a key on its input, so it gets a pipe to be told through.
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("starting {}", program.display()))?;
    let stdin = child.stdin.take().context("the recorder has no input to stop it by")?;
    Ok((child, stdin))
}

#[derive(Debug, PartialEq)]
enum Outcome {
    Recorded,
    Discarded,
    Empty,
}

/// Wait for the recording to end, by itself (its time limit) or because it was told to, and say
/// what there is. To stop, the recorder is first given a key, as someone at a terminal would
/// press one, then an interrupt, which it also finishes its file on, and killed only if it
/// answers neither.
async fn run((mut child, stdin): (Child, ChildStdin), stopped: oneshot::Receiver<End>, file: &Path) -> Outcome {
    let mut stdin = Some(stdin);
    let how = tokio::select! {
        _ = child.wait() => End::Keep,
        how = stopped => {
            // A sender dropped without a word (the app is going away) keeps nothing.
            let how = how.unwrap_or(End::Discard);
            if let Some(mut input) = stdin.take() {
                let _ = input.write_all(b"q\n").await;
                let _ = input.flush().await;
            }
            if tokio::time::timeout(AFTER_KEY, child.wait()).await.is_err() {
                interrupt(&child);
                if tokio::time::timeout(AFTER_SIGNAL, child.wait()).await.is_err() {
                    let _ = child.kill().await;
                }
            }
            how
        }
    };
    drop(stdin);
    let written = std::fs::metadata(file).map(|m| m.len() > 0).unwrap_or(false);
    match (how, written) {
        (End::Discard, _) => {
            let _ = std::fs::remove_file(file);
            Outcome::Discarded
        }
        (End::Keep, true) => Outcome::Recorded,
        (End::Keep, false) => {
            let _ = std::fs::remove_file(file);
            Outcome::Empty
        }
    }
}

/// Control-C, as far as the recorder can tell.
fn interrupt(child: &Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        // SAFETY: a signal to a child this process started and still holds.
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGINT);
        }
    }
    #[cfg(not(unix))]
    let _ = child;
}

/// What the app's URL scheme answers `path` (`/<token>`) with: the finished recording when the
/// token is its own, once, after which the file is gone; `None` for anything else.
fn take(recorder: &Recorder, path: &str) -> Option<Vec<u8>> {
    let asked = path.trim_start_matches('/');
    let mut finished = recorder.finished.lock().unwrap();
    if asked.is_empty() || finished.as_ref().is_none_or(|f| f.token != asked) {
        return None;
    }
    let file = finished.take()?.path;
    let bytes = std::fs::read(&file).ok();
    let _ = std::fs::remove_file(&file);
    bytes
}

/// Answer a request to `cedarrec://localhost/<token>`. The chat page is another origin, so the
/// answer says any origin may read it: the token is what keeps it to the page that was told.
pub fn serve(app: &AppHandle, request: &tauri::http::Request<Vec<u8>>) -> tauri::http::Response<Vec<u8>> {
    let respond = |status: u16, kind: &str, body: Vec<u8>| {
        tauri::http::Response::builder()
            .status(status)
            .header("Content-Type", kind)
            .header("Access-Control-Allow-Origin", "*")
            .header("Cache-Control", "no-store")
            .body(body)
            .expect("a response of fixed headers is valid")
    };
    let state = app.state::<AppState>();
    match take(state.recorder(), request.uri().path()) {
        Some(bytes) => respond(200, "video/quicktime", bytes),
        None => respond(404, "text/plain", b"no such recording".to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recorder_is_asked_for_a_video_of_the_whole_screen_with_a_time_limit() {
        assert_eq!(
            capture_args(Path::new("/tmp/a b.mov"), 600),
            ["-v", "-k", "-x", "-V", "600", "/tmp/a b.mov"]
        );
    }

    #[test]
    fn a_recording_is_handed_over_once_and_only_for_its_token() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("cedar-recorder-test-{}.mov", token()));
        std::fs::write(&path, b"movie").unwrap();
        let recorder = Recorder::default();
        *recorder.finished.lock().unwrap() = Some(Finished { token: "abc123".into(), path: path.clone() });

        assert_eq!(take(&recorder, "/"), None);
        assert_eq!(take(&recorder, "/other"), None);
        assert!(path.exists(), "a wrong token leaves the recording");
        assert_eq!(take(&recorder, "/abc123").as_deref(), Some(&b"movie"[..]));
        assert!(!path.exists(), "the file goes once it was fetched");
        assert_eq!(take(&recorder, "/abc123"), None);
    }

    /// A stand-in for screencapture: writes its file, then waits for a key or an interrupt.
    #[cfg(unix)]
    fn fake_recorder(script: &str) -> (PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("cedar-recorder-test-{}", token()));
        std::fs::create_dir_all(&dir).unwrap();
        let program = dir.join("fake-screencapture");
        std::fs::write(&program, format!("#!/bin/sh\nfor a; do f=$a; done\n{script}\n")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        (program, dir.join("out.mov"))
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_key_stops_the_recorder_and_the_file_is_kept_or_thrown_away() {
        // Finishes its file when a key arrives, as screencapture does.
        let (program, file) = fake_recorder("read key; echo movie > \"$f\"");
        let args = capture_args(&file, 60);

        let (stop, stopped) = oneshot::channel();
        let child = spawn(&program, &args).await.unwrap();
        stop.send(End::Keep).unwrap();
        assert_eq!(run(child, stopped, &file).await, Outcome::Recorded);
        assert!(file.exists());

        let (stop, stopped) = oneshot::channel();
        let child = spawn(&program, &args).await.unwrap();
        stop.send(End::Discard).unwrap();
        assert_eq!(run(child, stopped, &file).await, Outcome::Discarded);
        assert!(!file.exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_recorder_that_ignores_keys_is_interrupted() {
        // Reads nothing; finishes its file on an interrupt only.
        let (program, file) = fake_recorder("trap 'echo movie > \"$f\"; exit 0' INT; while :; do sleep 0.1; done");
        let (stop, stopped) = oneshot::channel();
        let child = spawn(&program, &capture_args(&file, 60)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        stop.send(End::Keep).unwrap();
        assert_eq!(run(child, stopped, &file).await, Outcome::Recorded);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_recording_ends_by_itself_and_one_that_wrote_nothing_is_empty() {
        let (program, file) = fake_recorder("echo movie > \"$f\"");
        let (_stop, stopped) = oneshot::channel();
        let child = spawn(&program, &capture_args(&file, 60)).await.unwrap();
        assert_eq!(run(child, stopped, &file).await, Outcome::Recorded, "its time limit ended it");

        // What the system does to a recorder it keeps from the screen: no file.
        let (program, file) = fake_recorder("exit 1");
        let (_stop, stopped) = oneshot::channel();
        let child = spawn(&program, &capture_args(&file, 60)).await.unwrap();
        assert_eq!(run(child, stopped, &file).await, Outcome::Empty);
    }
}
