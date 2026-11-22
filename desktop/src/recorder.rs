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
//!   /__cedar-app/record/stop            stop and hand the recording over (so does a click on
//!                                       the red light the app puts in the menu bar meanwhile,
//!                                       since its window hides while it records)
//!   /__cedar-app/record/cancel          stop and throw it away
//!
//! and hears back as `cedar-recorder` events on its window, each with a `detail`:
//!
//!   {"state": "recording"}
//!   {"state": "stopping"}                 the recorder was told to stop and is writing its file
//!   {"state": "done", "url": "cedarrec://localhost/<token>", "bytes": 1234567, "seconds": 42, "media_type": "video/quicktime"}
//!   {"state": "failed", "reason": "permission" | "empty" | "unavailable" | "busy", "message"?}
//!   {"state": "cancelled"}
//!
//! The page plays `url` in a video element: the app serves the file through its own URL scheme
//! (see [`serve`]), in ranges, since a recording of the whole screen is far too large to hand
//! over in one piece. The address holds a random token, so only the page that was told it can
//! read the recording. The page picks the recording's key frames from it and sends those to
//! cedar, as it does with one it recorded itself, and says `record/cancel` when it is done,
//! which deletes the file; so does the next recording.

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
/// A key is what makes it save: it then writes the file out, which takes longer the longer the
/// recording was. An interrupt ends it at once and WITHOUT a file (seen on macOS 26), so that
/// comes only when the key got no answer for a long time, to not leave a recorder running.
const AFTER_KEY: Duration = Duration::from_secs(if cfg!(test) { 1 } else { 120 });
const AFTER_SIGNAL: Duration = Duration::from_secs(15);
/// How long the file may take to appear once the recorder has exited.
const FILE_WAIT: Duration = Duration::from_secs(if cfg!(test) { 1 } else { 5 });

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
        // The recording should show what the person does, not Cedar: the window goes out of
        // the way, and a red light in the menu bar is there to stop it by. Both are undone
        // when the recording ends, however it ends.
        crate::window::hide(&app);
        crate::tray::show_recording(&app);
        let started = Instant::now();
        let outcome = run(child, stopped, &path).await;
        recorder.active.lock().unwrap().take();
        crate::tray::hide_recording(&app);
        if outcome != Outcome::Discarded {
            crate::window::reveal(&app);
        }
        match outcome {
            Outcome::Recorded => {
                let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                *recorder.finished.lock().unwrap() = Some(Finished { token: token.clone(), path });
                push(
                    &app,
                    json!({ "state": "done", "url": format!("{SCHEME}://localhost/{token}"), "bytes": bytes,
                            "seconds": started.elapsed().as_secs(), "media_type": "video/quicktime" }),
                );
            }
            Outcome::Discarded => push(&app, json!({ "state": "cancelled" })),
            // Nothing was written although the recorder ran; what it said goes to the page, and
            // to the app's own output for whoever looks there.
            Outcome::Empty(why) => {
                eprintln!("screen recording: no file. {why}");
                failed(&app, "empty", Some(why));
            }
        }
    });
}

/// Stop the recording under way and hand it to the page (`done` follows).
pub fn stop(app: &AppHandle) {
    if app.state::<AppState>().recorder().end(End::Keep) {
        // Asked for from the page or from the light in the menu bar: either way the page says
        // that the recording is being finished, and the window comes back to show it.
        push(app, json!({ "state": "stopping" }));
        crate::tray::hide_recording(app);
        crate::window::reveal(app);
    }
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
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
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
    /// No file came of it; with how the recorder ended and what it printed.
    Empty(String),
}

/// What the recorder printed, read to its end without holding the recorder up.
fn collect(stream: Option<impl tokio::io::AsyncRead + Unpin + Send + 'static>) -> tokio::task::JoinHandle<String> {
    tokio::spawn(async move {
        let mut text = String::new();
        if let Some(mut stream) = stream {
            use tokio::io::AsyncReadExt;
            let mut bytes = Vec::new();
            let _ = stream.read_to_end(&mut bytes).await;
            text = String::from_utf8_lossy(&bytes).trim().chars().take(600).collect();
        }
        text
    })
}

/// Wait for the recording to end, by itself (its time limit) or because it was told to, and say
/// what there is. To stop, the recorder is given a key, as someone at a terminal would press
/// one, and the time to write its file. Only one that doesn't react to that is interrupted
/// (which loses the recording) and then killed.
async fn run((mut child, stdin): (Child, ChildStdin), stopped: oneshot::Receiver<End>, file: &Path) -> Outcome {
    let mut stdin = Some(stdin);
    let started = Instant::now();
    let (out, err) = (collect(child.stdout.take()), collect(child.stderr.take()));
    // How it came to end, for the report when no file comes of it.
    let mut ended = "it ended by itself";
    let how = tokio::select! {
        _ = child.wait() => End::Keep,
        how = stopped => {
            // A sender dropped without a word (the app is going away) keeps nothing.
            let how = how.unwrap_or(End::Discard);
            if let Some(mut input) = stdin.take() {
                let _ = input.write_all(b"q\n").await;
                let _ = input.flush().await;
            }
            ended = "it stopped at a key";
            if tokio::time::timeout(AFTER_KEY, child.wait()).await.is_err() {
                interrupt(&child);
                ended = "it stopped at an interrupt";
                if tokio::time::timeout(AFTER_SIGNAL, child.wait()).await.is_err() {
                    let _ = child.kill().await;
                    ended = "it had to be killed";
                }
            }
            how
        }
    };
    drop(stdin);
    let status = child.wait().await.map(|s| s.to_string()).unwrap_or_else(|e| e.to_string());
    let ran = started.elapsed();
    let written = || std::fs::metadata(file).map(|m| m.len() > 0).unwrap_or(false);
    if how == End::Discard {
        let _ = std::fs::remove_file(file);
        return Outcome::Discarded;
    }
    // The recorder may hand the writing to a helper that finishes a moment after it exits.
    let deadline = Instant::now() + FILE_WAIT;
    while !written() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    if written() {
        return Outcome::Recorded;
    }
    let _ = std::fs::remove_file(file);
    let said = [out.await.unwrap_or_default(), err.await.unwrap_or_default()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");
    Outcome::Empty(format!(
        "screencapture ran {:.1}s, {ended} ({status}), and wrote no file{}",
        ran.as_secs_f32(),
        if said.is_empty() { "; it printed nothing".to_string() } else { format!("; it printed: {said}") }
    ))
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

/// The most of a recording one answer carries. A recording of the whole screen runs to megabytes
/// a second, so the page's video element reads it in ranges, as it would from a web server,
/// and nothing ever holds all of it in memory.
const MAX_PART: u64 = 8 << 20;

/// A part of the finished recording: its bytes, where they start, and how long the whole is.
#[derive(Debug, PartialEq)]
struct Part {
    bytes: Vec<u8>,
    start: u64,
    total: u64,
    /// Whether a range was asked for (the answer is then a 206).
    ranged: bool,
}

/// The first and last byte a `Range` header asks for (`bytes=100-`, `bytes=0-1`), within a file
/// of `total` bytes and at most [`MAX_PART`] long. `None`: not a range this understands, or one
/// outside the file.
fn byte_range(header: &str, total: u64) -> Option<(u64, u64)> {
    let (from, to) = header.trim().strip_prefix("bytes=")?.split_once('-')?;
    let last = total.checked_sub(1)?;
    let (start, end) = match (from.trim(), to.trim()) {
        // The last so many bytes.
        ("", n) => (total.saturating_sub(n.parse::<u64>().ok()?), last),
        (s, "") => (s.parse().ok()?, last),
        (s, e) => (s.parse().ok()?, e.parse::<u64>().ok()?.min(last)),
    };
    (start <= end && start <= last).then(|| (start, end.min(start + MAX_PART - 1)))
}

/// What the app's URL scheme answers `path` (`/<token>`) with: the finished recording, or the
/// `range` of it asked for, when the token is its own; `None` for anything else. The file stays
/// until the page says it is done with it (`record/cancel`) or another recording starts.
fn part(recorder: &Recorder, path: &str, range: Option<&str>) -> Option<Part> {
    use std::io::{Read, Seek, SeekFrom};
    let asked = path.trim_start_matches('/');
    let finished = recorder.finished.lock().unwrap();
    let file = &finished.as_ref().filter(|f| !asked.is_empty() && f.token == asked)?.path;
    let mut file = std::fs::File::open(file).ok()?;
    let total = file.metadata().ok()?.len();
    let (start, end, ranged) = match range {
        Some(header) => {
            let (start, end) = byte_range(header, total)?;
            (start, end, true)
        }
        None => (0, total.checked_sub(1)?, false),
    };
    let mut bytes = vec![0; usize::try_from(end - start + 1).ok()?];
    file.seek(SeekFrom::Start(start)).ok()?;
    file.read_exact(&mut bytes).ok()?;
    Some(Part { bytes, start, total, ranged })
}

/// Answer a request to `cedarrec://localhost/<token>`. The chat page is another origin, so the
/// answer says any origin may read it (the page draws the video's frames, which a browser only
/// allows of a video it was let read): the token is what keeps it to the page that was told.
pub fn serve(app: &AppHandle, request: &tauri::http::Request<Vec<u8>>) -> tauri::http::Response<Vec<u8>> {
    let respond = |status: u16, kind: &str, range: Option<String>, body: Vec<u8>| {
        let mut response = tauri::http::Response::builder()
            .status(status)
            .header("Content-Type", kind)
            .header("Content-Length", body.len().to_string())
            .header("Accept-Ranges", "bytes")
            .header("Access-Control-Allow-Origin", "*")
            .header("Access-Control-Expose-Headers", "Content-Range, Content-Length, Accept-Ranges")
            .header("Cache-Control", "no-store");
        if let Some(range) = range {
            response = response.header("Content-Range", range);
        }
        response.body(body).expect("a response of fixed headers is valid")
    };
    let state = app.state::<AppState>();
    let range = request.headers().get("range").and_then(|v| v.to_str().ok());
    match part(state.recorder(), request.uri().path(), range) {
        Some(p) if p.ranged => {
            let range = format!("bytes {}-{}/{}", p.start, p.start + p.bytes.len() as u64 - 1, p.total);
            respond(206, "video/quicktime", Some(range), p.bytes)
        }
        Some(p) => respond(200, "video/quicktime", None, p.bytes),
        None => respond(404, "text/plain", None, b"no such recording".to_vec()),
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
    fn a_recording_is_read_in_ranges_and_only_for_its_token_until_it_is_let_go() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("cedar-recorder-test-{}.mov", token()));
        std::fs::write(&path, b"0123456789").unwrap();
        let recorder = Recorder::default();
        *recorder.finished.lock().unwrap() = Some(Finished { token: "abc123".into(), path: path.clone() });

        assert_eq!(part(&recorder, "/", None), None);
        assert_eq!(part(&recorder, "/other", Some("bytes=0-1")), None);
        let whole = part(&recorder, "/abc123", None).unwrap();
        assert_eq!((whole.bytes.as_slice(), whole.ranged, whole.total), (&b"0123456789"[..], false, 10));
        let range = |r: &str| part(&recorder, "/abc123", Some(r)).map(|p| (String::from_utf8(p.bytes).unwrap(), p.start));
        assert_eq!(range("bytes=0-1"), Some(("01".into(), 0)));
        assert_eq!(range("bytes=7-"), Some(("789".into(), 7)));
        assert_eq!(range("bytes=8-99"), Some(("89".into(), 8)));
        assert_eq!(range("bytes=-3"), Some(("789".into(), 7)));
        assert_eq!(range("bytes=10-"), None);
        assert_eq!(range("lines=1-2"), None);
        // Read as often as the video element likes; gone when the page is done with it.
        assert!(path.exists());
        recorder.discard_finished();
        assert!(!path.exists());
        assert_eq!(part(&recorder, "/abc123", None), None);

        // An open range of a long file is answered a part at a time.
        assert_eq!(byte_range("bytes=5-", 100 << 20), Some((5, 5 + MAX_PART - 1)));
    }

    /// Start a stand-in recorder. Tests run side by side, and one that forks while another is
    /// still writing its script makes that script "busy" for a moment: try again then.
    #[cfg(unix)]
    async fn start(program: &Path, args: &[String]) -> (Child, ChildStdin) {
        for _ in 0..50 {
            match spawn(program, args).await {
                Ok(child) => return child,
                Err(e) if format!("{e:#}").contains("busy") => tokio::time::sleep(Duration::from_millis(20)).await,
                Err(e) => panic!("{e:#}"),
            }
        }
        panic!("the stand-in recorder stayed busy");
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
        let child = start(&program, &args).await;
        stop.send(End::Keep).unwrap();
        assert_eq!(run(child, stopped, &file).await, Outcome::Recorded);
        assert!(file.exists());

        let (stop, stopped) = oneshot::channel();
        let child = start(&program, &args).await;
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
        let child = start(&program, &capture_args(&file, 60)).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        stop.send(End::Keep).unwrap();
        assert_eq!(run(child, stopped, &file).await, Outcome::Recorded);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_recording_ends_by_itself_and_one_that_wrote_nothing_is_empty() {
        let (program, file) = fake_recorder("echo movie > \"$f\"");
        let (_stop, stopped) = oneshot::channel();
        let child = start(&program, &capture_args(&file, 60)).await;
        assert_eq!(run(child, stopped, &file).await, Outcome::Recorded, "its time limit ended it");

        // What the system does to a recorder it keeps from the screen: no file.
        let (program, file) = fake_recorder("echo no screen for you >&2; exit 1");
        let (_stop, stopped) = oneshot::channel();
        let child = start(&program, &capture_args(&file, 60)).await;
        let Outcome::Empty(why) = run(child, stopped, &file).await else { panic!("a file?") };
        assert!(why.contains("it ended by itself (exit status: 1)") && why.contains("it printed: no screen for you"), "{why}");
    }
}
