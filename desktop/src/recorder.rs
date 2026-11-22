//! Recording the screen for the chat's Actions page (chat/static/actions.js), on a Mac.
//!
//! A page in a browser records the screen itself (`getDisplayMedia`). The app's window can't on a
//! Mac: WKWebView offers no screen sharing. So the app records for it, with macOS's
//! ScreenCaptureKit (see `recorder/capture.rs`): a stream of the main display that the system
//! writes into a movie file, started and stopped by the app itself, under the system's Screen
//! Recording permission for Cedar. That takes macOS 15; on an older one the page offers a video
//! file instead. On Windows the window (WebView2) records like a browser, so none of this is
//! used there.
//!
//! The page asks through the window (see `window::PageAction`):
//!
//!   /__cedar-app/record/displays        say which displays there are (`displays` follows)
//!   /__cedar-app/record/start?max=600&display=2
//!                                       record a display (that one of `displays`, or the main
//!                                       one), for at most that many seconds
//!   /__cedar-app/record/stop            stop and hand the recording over (so does a click on
//!                                       the red light the app puts in the menu bar meanwhile,
//!                                       since its window hides while it records)
//!   /__cedar-app/record/cancel          stop and throw it away
//!
//! and hears back as `cedar-recorder` events on its window, each with a `detail`:
//!
//!   {"state": "displays", "displays": [{"id": 2, "name": "Studio Display", "width": 2560, "height": 1440, "main": true}]}
//!   {"state": "recording"}
//!   {"state": "stopping"}                 the recording was told to stop and its file is being closed
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
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

#[cfg(target_os = "macos")]
mod capture;

/// Where nothing records for the page: it is never asked to (see [`Recorder::available`]).
#[cfg(not(target_os = "macos"))]
mod capture {
    use super::{Display, End, Outcome};

    pub fn available() -> bool {
        false
    }

    pub fn screens() -> Vec<Display> {
        Vec::new()
    }

    pub fn record(
        _file: &std::path::Path,
        _display: Option<u32>,
        _max: std::time::Duration,
        _stopped: std::sync::mpsc::Receiver<End>,
        _started: impl FnOnce(),
    ) -> Outcome {
        Outcome::Empty("this computer's window records by itself".into())
    }
}

/// The URL scheme recordings are handed to the page through.
pub const SCHEME: &str = "cedarrec";
/// How long after Cedar's window has gone the recording starts: long enough for macOS to have
/// drawn the screen without it, so the first frame shows what the person sees and not Cedar.
const AFTER_HIDING: Duration = Duration::from_millis(200);
/// The longest recording when the page names none, and the longest it may name, in seconds.
const DEFAULT_MAX_SECS: u64 = 600;
const LIMIT_MAX_SECS: u64 = 1800;
/// A display that can be recorded: the number the system knows it by, the name System Settings
/// shows, and its size in points. `main` is the one with the menu bar.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Display {
    id: u32,
    name: String,
    width: u32,
    height: u32,
    main: bool,
}

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
    active: Mutex<Option<Sender<End>>>,
    finished: Mutex<Option<Finished>>,
}

impl Recorder {
    /// Whether this app records for its chat page: on a Mac whose system can record to a file.
    pub fn available() -> bool {
        capture::available()
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

fn token() -> String {
    use rand::RngCore;
    let mut buf = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    hex::encode(buf)
}

/// Tell the page which displays there are, for it to offer the choice when there are several.
/// Called from the window's navigation handler, on the main thread, where the system lists them.
pub fn displays(app: &AppHandle) {
    push(app, json!({ "state": "displays", "displays": capture::screens() }));
}

/// Start recording a display: `display` of [`displays`], or the main one. Answers the page with
/// `recording`, or `failed` and why.
pub fn start(app: &AppHandle, max_secs: Option<u64>, display: Option<u32>) {
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
    let (stop, stopped) = channel();
    *recorder.active.lock().unwrap() = Some(stop);
    let app = app.clone();
    // This is called from the window's navigation handler, on the main thread; the recording
    // waits on the system from start to finish, so it gets a thread of its own.
    std::thread::spawn(move || {
        let state = app.state::<AppState>();
        let recorder = state.recorder();
        let mut started = None;
        // The recording should show what the person does, not Cedar: the window goes out of the
        // way first, and the recording starts once the screen shows what was behind it. A red
        // light in the menu bar is there to stop it by. Both are undone when the recording
        // ends, however it ends.
        crate::window::hide_and_wait(&app);
        std::thread::sleep(AFTER_HIDING);
        let outcome = capture::record(&path, display, Duration::from_secs(max_secs), stopped, || {
            started = Some(Instant::now());
            push(&app, json!({ "state": "recording" }));
            crate::tray::show_recording(&app);
        });
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
                            "seconds": started.map(|at| at.elapsed().as_secs()).unwrap_or(0), "media_type": "video/quicktime" }),
                );
            }
            Outcome::Discarded => push(&app, json!({ "state": "cancelled" })),
            // No recording came of it; why goes to the page, and to the app's own output for
            // whoever looks there.
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

#[derive(Debug, PartialEq)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
enum Outcome {
    Recorded,
    Discarded,
    /// No file came of it; with how the recording ended and what the system said.
    Empty(String),
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
}
