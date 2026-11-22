//! The recording itself, with macOS's ScreenCaptureKit: a stream of the main display with a
//! recording output, which writes the movie file (macOS 15 and later).
//!
//! Everything here runs on one thread of its own, start to finish ([`record`] blocks until the
//! recording is over). The system calls back on its own queues; each callback only puts a word
//! into a channel that thread waits on, so none of the system's objects change threads except
//! the list of displays, on its way in.

use super::{Display, End, Outcome};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_core_media::CMTime;
use objc2_foundation::{NSArray, NSDictionary, NSError, NSNumber, NSObject, NSObjectProtocol, NSRect, NSString, NSURL};
use objc2_screen_capture_kit::{
    SCContentFilter, SCRecordingOutput, SCRecordingOutputConfiguration, SCRecordingOutputDelegate, SCShareableContent,
    SCDisplay, SCStream, SCStreamConfiguration,
};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// How long the system gets to list the displays, and to start the stream.
const SETUP: Duration = Duration::from_secs(15);
/// How long it gets to finish the file once told to stop: the movie is written as it is
/// recorded, so this is closing it, not writing it out.
const FINISH: Duration = Duration::from_secs(30);
/// The longer side of the recording, in pixels: enough to read a screen by, and a fraction of
/// a Retina display's own size.
const MAX_SIDE: f64 = 1600.0;
/// Frames a second. Someone clicking through a task doesn't need more, and the file stays small.
const FPS: i32 = 5;

/// Whether this macOS can record to a file through ScreenCaptureKit (15 and later).
pub fn available() -> bool {
    AnyClass::get(c"SCRecordingOutput").is_some() && AnyClass::get(c"SCStream").is_some()
}

/// The displays connected, the one with the menu bar first, with the names System Settings
/// shows them under. Asks AppKit, so only on the main thread.
pub fn screens() -> Vec<Display> {
    let Some(class) = AnyClass::get(c"NSScreen") else {
        return Vec::new();
    };
    // SAFETY: `+[NSScreen screens]` returns an array of screens; of each, `localizedName` is a
    // string, `frame` a rectangle in points, and `deviceDescription` a dictionary whose
    // "NSScreenNumber" is the number ScreenCaptureKit knows the display by.
    let screens: Retained<NSArray<NSObject>> = unsafe { msg_send![class, screens] };
    let key = NSString::from_str("NSScreenNumber");
    let mut found = Vec::new();
    for (at, screen) in screens.iter().enumerate() {
        let name: Retained<NSString> = unsafe { msg_send![&*screen, localizedName] };
        let frame: NSRect = unsafe { msg_send![&*screen, frame] };
        let about: Retained<NSDictionary<NSString, NSObject>> = unsafe { msg_send![&*screen, deviceDescription] };
        let Some(number) = about.objectForKey(&key).and_then(|n| n.downcast::<NSNumber>().ok()) else {
            continue;
        };
        found.push(Display {
            id: number.unsignedIntValue(),
            name: name.to_string(),
            width: frame.size.width.round() as u32,
            height: frame.size.height.round() as u32,
            main: at == 0,
        });
    }
    found
}

/// An object of the system's handed from the queue it arrived on to the recording's thread.
struct Handed<T>(T);
// SAFETY: used for the list of displays only, which the system hands to a completion handler on
// a queue of its choosing for the caller to use wherever it likes; it is immutable, and only
// the recording's thread touches it after the handover.
unsafe impl<T> Send for Handed<T> {}

/// What the system says of the file: finished, or failed and why.
type Word = Result<(), String>;

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this class implements no `Drop`.
    #[unsafe(super(NSObject))]
    #[name = "CedarRecordingDelegate"]
    #[ivars = Sender<Word>]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl SCRecordingOutputDelegate for Delegate {
        #[unsafe(method(recordingOutput:didFailWithError:))]
        fn failed(&self, _output: &SCRecordingOutput, error: &NSError) {
            let _ = self.ivars().send(Err(describe(error)));
        }

        #[unsafe(method(recordingOutputDidFinishRecording:))]
        fn finished(&self, _output: &SCRecordingOutput) {
            let _ = self.ivars().send(Ok(()));
        }
    }
);

impl Delegate {
    fn new(says: Sender<Word>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(says);
        // SAFETY: NSObject's `init`, on a freshly allocated object whose ivars are set.
        unsafe { msg_send![super(this), init] }
    }
}

fn describe(error: &NSError) -> String {
    format!("{} ({} {})", error.localizedDescription(), error.domain(), error.code())
}

/// A completion handler that only has an error to report, or none.
fn reports(to: Sender<Word>) -> RcBlock<dyn Fn(*mut NSError)> {
    RcBlock::new(move |error: *mut NSError| {
        // SAFETY: the system passes nil or an error that is valid for the length of the call.
        let word = match unsafe { error.as_ref() } {
            Some(error) => Err(describe(error)),
            None => Ok(()),
        };
        let _ = to.send(word);
    })
}

fn heard(from: &Receiver<Word>, within: Duration, what: &str) -> Word {
    match from.recv_timeout(within) {
        Ok(word) => word.map_err(|e| format!("{what}: {e}")),
        Err(_) => Err(format!("{what}: macOS gave no answer in {}s", within.as_secs())),
    }
}

/// The displays there are, asked of the system and waited for.
fn displays() -> Result<Retained<SCShareableContent>, String> {
    let (says, said) = channel::<Result<Handed<Retained<SCShareableContent>>, String>>();
    let handler = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
        // SAFETY: the system passes nil or valid objects; retaining one keeps it past the call.
        let word = match unsafe { Retained::retain(content) } {
            Some(content) => Ok(Handed(content)),
            None => Err(unsafe { error.as_ref() }.map(describe).unwrap_or_else(|| "no displays were listed".into())),
        };
        let _ = says.send(word);
    });
    // SAFETY: the handler is a block of the signature this method takes, and is copied by it.
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&handler) };
    match said.recv_timeout(SETUP) {
        Ok(word) => word.map(|handed| handed.0).map_err(|e| format!("listing the displays: {e}")),
        Err(_) => Err(format!("listing the displays: macOS gave no answer in {}s", SETUP.as_secs())),
    }
}

/// The size to record a display of `width` by `height` at: its own, or smaller in proportion
/// when its longer side is over [`MAX_SIDE`]. Even numbers, as video encoders want them.
pub(super) fn frame_size(width: f64, height: f64) -> (usize, usize) {
    let scale = (MAX_SIDE / width.max(height).max(1.0)).min(1.0);
    let even = |side: f64| (((side * scale).round() as usize).max(2) / 2) * 2;
    (even(width), even(height))
}

/// Record a display into `file` (the one of that number, or the main one) until `stopped` says how to end, or for `max` at the
/// most. `started` is called once the recording runs. Blocks; call it on a thread of its own.
pub fn record(file: &Path, display: Option<u32>, max: Duration, stopped: Receiver<End>, started: impl FnOnce()) -> Outcome {
    let began = Instant::now();
    let empty = |why: String| {
        let _ = std::fs::remove_file(file);
        Outcome::Empty(format!("{why}, after {:.1}s", began.elapsed().as_secs_f32()))
    };
    let content = match displays() {
        Ok(content) => content,
        Err(e) => return empty(e),
    };
    // SAFETY (for all the calls below): plain calls into ScreenCaptureKit with valid objects of
    // the types its methods take, all made and kept on this thread.
    // The display asked for; without one, or when it has gone since the page listed it, the
    // first, which is the main one.
    let all = unsafe { content.displays() };
    // SAFETY: `displayID` takes no arguments and returns the display's number.
    let number = |d: &SCDisplay| -> u32 { unsafe { msg_send![d, displayID] } };
    let wanted = display.and_then(|id| all.iter().find(|d| number(d) == id));
    let Some(display) = wanted.or_else(|| all.firstObject()) else {
        return empty("no display to record".into());
    };
    let filter = unsafe {
        SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &display, &NSArray::new())
    };
    let config = unsafe { SCStreamConfiguration::new() };
    let (width, height) = frame_size(unsafe { display.width() } as f64, unsafe { display.height() } as f64);
    unsafe {
        config.setWidth(width);
        config.setHeight(height);
        config.setMinimumFrameInterval(CMTime::new(1, FPS));
        config.setShowsCursor(true);
        // So whoever watches sees what was pressed.
        config.setShowMouseClicks(true);
        config.setCapturesAudio(false);
    }
    let stream = unsafe { SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &config, None) };

    let (file_says, file_said) = channel::<Word>();
    let delegate = Delegate::new(file_says);
    let output_config = unsafe { SCRecordingOutputConfiguration::new() };
    unsafe {
        output_config.setOutputURL(&NSURL::fileURLWithPath(&NSString::from_str(&file.to_string_lossy())));
    }
    let output = unsafe {
        SCRecordingOutput::initWithConfiguration_delegate(
            SCRecordingOutput::alloc(),
            &output_config,
            ProtocolObject::from_ref(&*delegate),
        )
    };
    if let Err(e) = unsafe { stream.addRecordingOutput_error(&output) } {
        return empty(format!("setting up the file: {}", describe(&e)));
    }

    let (start_says, start_said) = channel::<Word>();
    unsafe { stream.startCaptureWithCompletionHandler(Some(&reports(start_says))) };
    if let Err(e) = heard(&start_said, SETUP, "starting the recording") {
        return empty(e);
    }
    started();

    // Until told to stop, the time is up, or the file fails under way.
    let deadline = Instant::now() + max;
    let mut ended = "it was stopped";
    let mut failure = None;
    let how = loop {
        if let Ok(Err(e)) = file_said.try_recv() {
            failure = Some(e);
            break End::Keep;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            ended = "its time was up";
            break End::Keep;
        }
        match stopped.recv_timeout(left.min(Duration::from_millis(250))) {
            Ok(how) => break how,
            Err(RecvTimeoutError::Timeout) => {}
            // Whoever could stop it is gone (the app is going away): keep nothing.
            Err(RecvTimeoutError::Disconnected) => break End::Discard,
        }
    };

    let (stop_says, stop_said) = channel::<Word>();
    unsafe { stream.stopCaptureWithCompletionHandler(Some(&reports(stop_says))) };
    let stopping = heard(&stop_said, FINISH, "stopping the recording");
    if how == End::Discard {
        // Give the file a moment to be closed, so that removing it removes all of it.
        let _ = file_said.recv_timeout(Duration::from_secs(2));
        let _ = std::fs::remove_file(file);
        return Outcome::Discarded;
    }
    // The file is whole once the system says so; a stream that stopped cleanly says it soon.
    let finishing = match failure {
        Some(e) => Err(e),
        None => heard(&file_said, FINISH, "finishing the file"),
    };
    drop((output, delegate, stream));
    let written = std::fs::metadata(file).map(|m| m.len() > 0).unwrap_or(false);
    match (finishing, written) {
        (Ok(()), true) => Outcome::Recorded,
        (finishing, _) => {
            let mut why = format!("the recording ran {:.1}s, {ended}", began.elapsed().as_secs_f32());
            for problem in [stopping.err(), finishing.err()].into_iter().flatten() {
                why.push_str(&format!("; {problem}"));
            }
            if !written {
                why.push_str("; no file was written");
            }
            let _ = std::fs::remove_file(file);
            Outcome::Empty(why)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_large_display_is_recorded_smaller_in_proportion_and_a_small_one_as_it_is() {
        assert_eq!(frame_size(1512.0, 982.0), (1512, 982));
        assert_eq!(frame_size(3200.0, 1800.0), (1600, 900));
        assert_eq!(frame_size(1080.0, 1920.0), (900, 1600));
        assert_eq!(frame_size(1601.0, 1001.0), (1600, 1000));
    }
}
