//! `cedar computer ...`: the driver of the computer cedar itself runs on, for the `computer`
//! tool with `COMPUTER_RUNTIME=native`. It speaks what the container's driver speaks
//! (`computer/driver.py`), so everything above it is shared:
//!
//!     cedar computer info         {"screen": [width, height]}, or why the screen can't be used
//!     cedar computer call JSON    do one thing on the screen and print what it looks like after
//!
//! Positions are in the units the mouse is moved in (points on a Mac, where a Retina screen has
//! twice as many pixels), from the top left corner of the main display; the screenshot is the
//! main display scaled to that size, or to the request's `width` and `height`, as a JPEG. There
//! is no `open` (the workspace is not on this computer) and the reply names no window.
//!
//! Every call is a process of its own, started by the server (see
//! [`crate::computer::NativeComputerFactory`]), so one that hangs can be stopped. On a Mac the
//! screen needs two permissions for the application cedar belongs to: Screen Recording, to see
//! it, and Accessibility, to use the mouse and keyboard. A failure prints its reason to stderr
//! and exits 1.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use enigo::{Axis, Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::DynamicImage;
use serde_json::{json, Value};
use std::thread::sleep;
use std::time::Duration;

/// How long the screen gets to react before it is photographed.
const SETTLE: Duration = Duration::from_millis(600);
const MAX_WAIT_SECS: f64 = 10.0;
const MAX_TEXT_CHARS: usize = 10_000;
const MAX_KEYS: usize = 20;
const MAX_SCROLL: i64 = 30;
const JPEG_QUALITY: u8 = 80;
const NO_INPUT: &str = "Cedar may not use the mouse and keyboard yet: allow it under System \
     Settings → Privacy & Security → Accessibility, then try again. If Cedar is already switched \
     on there, the entry belongs to another build of the app: select it, remove it with the − \
     button, quit and reopen Cedar, and allow it again when asked";
#[cfg(target_os = "macos")]
const NO_SCREEN: &str = "Cedar may not see the screen yet: allow it under System Settings → \
     Privacy & Security → Screen Recording, then quit and reopen Cedar. If Cedar is already \
     switched on there, the entry belongs to another build of the app: remove it with the − \
     button first";

/// Run `cedar computer <args>`; the process's exit code.
pub fn main(args: &[String]) -> i32 {
    let done = match args {
        [command] if command == "info" => info(),
        [command, request] if command == "call" => serde_json::from_str(request)
            .context("the request is not JSON")
            .and_then(|request| call(&request)),
        _ => Err(anyhow!("usage: cedar computer info | cedar computer call JSON")),
    };
    match done {
        Ok(reply) => {
            println!("{reply}");
            0
        }
        Err(e) => {
            eprintln!("{e:#}");
            1
        }
    }
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

/// Whether the screen may be captured; on a Mac, asking for the permission when it is missing.
fn check_screen_access() -> Result<()> {
    #[cfg(target_os = "macos")]
    // SAFETY: both take no arguments and only read (or ask for) the process's permission.
    unsafe {
        if !CGPreflightScreenCaptureAccess() {
            CGRequestScreenCaptureAccess();
            bail!(NO_SCREEN);
        }
    }
    Ok(())
}

/// The mouse and keyboard. On a Mac, creating it asks for the Accessibility permission when it
/// is missing.
fn input() -> Result<Enigo> {
    Enigo::new(&Settings::default()).map_err(|e| match e {
        enigo::NewConError::NoPermission => anyhow!(NO_INPUT),
        other => anyhow!("the mouse and keyboard can't be used: {other}"),
    })
}

/// The size of the main display, in the units the mouse is moved in.
fn screen_size(enigo: &Enigo) -> Result<(u32, u32)> {
    let (width, height) = enigo
        .main_display()
        .map_err(|e| anyhow!("the screen's size is unknown: {e}"))?;
    match (u32::try_from(width), u32::try_from(height)) {
        (Ok(width), Ok(height)) if width >= 16 && height >= 16 => Ok((width, height)),
        _ => bail!("the screen's size is unknown"),
    }
}

fn info() -> Result<Value> {
    check_screen_access()?;
    let screen = screen_size(&input()?)?;
    Ok(json!({ "screen": [screen.0, screen.1] }))
}

fn call(request: &Value) -> Result<Value> {
    check_screen_access()?;
    let mut enigo = input()?;
    let screen = screen_size(&enigo)?;
    let side = |name: &str, max: u32| -> Result<u32> {
        match &request[name] {
            Value::Null => Ok(max),
            v => v
                .as_u64()
                .filter(|n| (16..=u64::from(max)).contains(n))
                .map(|n| n as u32)
                .ok_or_else(|| anyhow!("`{name}` must be a whole number from 16 to {max}")),
        }
    };
    let (width, height) = (side("width", screen.0)?, side("height", screen.1)?);
    if act(&mut enigo, request, screen)? {
        sleep(SETTLE);
    }
    let jpeg = screenshot(width, height)?;
    let pointer = enigo
        .location()
        .map_err(|e| anyhow!("the pointer's position is unknown: {e}"))?;
    Ok(json!({
        "image": base64::engine::general_purpose::STANDARD.encode(jpeg),
        "media_type": "image/jpeg",
        "width": width,
        "height": height,
        "screen": [screen.0, screen.1],
        "pointer": [pointer.0.max(0), pointer.1.max(0)],
        "window": null,
    }))
}

fn fail(what: &str) -> impl Fn(enigo::InputError) -> anyhow::Error + '_ {
    move |e| anyhow!("{what} failed: {e}")
}

/// A position on the screen from the request.
fn point(request: &Value, screen: (u32, u32), x: &str, y: &str) -> Result<(i32, i32)> {
    let within = |name: &str, max: u32| -> Result<i32> {
        request[name]
            .as_u64()
            .filter(|at| *at < u64::from(max))
            .map(|at| at as i32)
            .ok_or_else(|| anyhow!("`{name}` must be a whole number from 0 to {}", max - 1))
    };
    Ok((within(x, screen.0)?, within(y, screen.1)?))
}

/// Carry out the request's action; whether the screen should get a moment to settle after it.
fn act(enigo: &mut Enigo, request: &Value, screen: (u32, u32)) -> Result<bool> {
    let action = request["action"].as_str().unwrap_or_default();
    match action {
        "screenshot" => return Ok(false),
        "wait" => {
            let seconds = match &request["seconds"] {
                Value::Null => 1.0,
                v => v.as_f64().ok_or_else(|| anyhow!("`seconds` must be a number"))?,
            };
            sleep(Duration::from_secs_f64(seconds.clamp(0.0, MAX_WAIT_SECS)));
            return Ok(false);
        }
        "click" => {
            let button = match request["button"].as_str().unwrap_or("left") {
                "left" => Button::Left,
                "right" => Button::Right,
                "middle" => Button::Middle,
                _ => bail!("`button` must be left, right or middle"),
            };
            let count = match &request["count"] {
                Value::Null => 1,
                v => v
                    .as_u64()
                    .filter(|n| (1..=3).contains(n))
                    .ok_or_else(|| anyhow!("`count` must be a whole number from 1 to 3"))?,
            };
            let (x, y) = point(request, screen, "x", "y")?;
            enigo.move_mouse(x, y, Coordinate::Abs).map_err(fail("moving the pointer"))?;
            for n in 0..count {
                if n > 0 {
                    sleep(Duration::from_millis(80));
                }
                enigo.button(button, Direction::Click).map_err(fail("the click"))?;
            }
        }
        "move" => {
            let (x, y) = point(request, screen, "x", "y")?;
            enigo.move_mouse(x, y, Coordinate::Abs).map_err(fail("moving the pointer"))?;
        }
        "drag" => {
            let (x, y) = point(request, screen, "x", "y")?;
            let (to_x, to_y) = point(request, screen, "to_x", "to_y")?;
            let moving = fail("the drag");
            enigo.move_mouse(x, y, Coordinate::Abs).map_err(&moving)?;
            enigo.button(Button::Left, Direction::Press).map_err(&moving)?;
            sleep(Duration::from_millis(150));
            // A stop on the way, since many applications only start a drag once the pointer has
            // moved with the button down. The button is let go whatever happens on the way.
            let dragged = enigo
                .move_mouse((x + to_x) / 2, (y + to_y) / 2, Coordinate::Abs)
                .and_then(|()| {
                    sleep(Duration::from_millis(100));
                    enigo.move_mouse(to_x, to_y, Coordinate::Abs)
                });
            sleep(Duration::from_millis(150));
            enigo.button(Button::Left, Direction::Release).map_err(&moving)?;
            dragged.map_err(&moving)?;
        }
        "type" => {
            let text = request["text"]
                .as_str()
                .filter(|t| !t.is_empty())
                .ok_or_else(|| anyhow!("type needs `text`"))?;
            if text.chars().count() > MAX_TEXT_CHARS {
                bail!("`text` is longer than {MAX_TEXT_CHARS} characters; type it in parts");
            }
            // A line break is the Enter key, as in the container's driver.
            for (n, line) in text.split('\n').enumerate() {
                if n > 0 {
                    enigo.key(Key::Return, Direction::Click).map_err(fail("typing"))?;
                }
                if !line.is_empty() {
                    enigo.text(line).map_err(fail("typing"))?;
                }
            }
        }
        "key" => {
            let presses: Vec<&str> = request["keys"].as_str().unwrap_or_default().split_whitespace().collect();
            if presses.is_empty() || presses.len() > MAX_KEYS {
                bail!("key needs `keys`: names like Return, Escape, ctrl+s or alt+F4, at most {MAX_KEYS}, separated by spaces");
            }
            // Checked as a whole before the first key goes down.
            let combos = presses.iter().map(|p| combo(p)).collect::<Result<Vec<_>>>()?;
            for keys in combos {
                press(enigo, &keys)?;
                sleep(Duration::from_millis(60));
            }
        }
        "scroll" => {
            let (axis, sign) = match request["direction"].as_str().unwrap_or("down") {
                "down" => (Axis::Vertical, 1),
                "up" => (Axis::Vertical, -1),
                "right" => (Axis::Horizontal, 1),
                "left" => (Axis::Horizontal, -1),
                _ => bail!("`direction` must be up, down, left or right"),
            };
            let amount = match &request["amount"] {
                Value::Null => 3,
                v => v
                    .as_i64()
                    .filter(|n| (1..=MAX_SCROLL).contains(n))
                    .ok_or_else(|| anyhow!("`amount` must be a whole number from 1 to {MAX_SCROLL}"))?,
            };
            if !request["x"].is_null() || !request["y"].is_null() {
                let (x, y) = point(request, screen, "x", "y")?;
                enigo.move_mouse(x, y, Coordinate::Abs).map_err(fail("moving the pointer"))?;
            }
            enigo.scroll(sign * amount as i32, axis).map_err(fail("scrolling"))?;
        }
        "open" => bail!("open is not available on this computer; use its file manager or an application's Open dialog"),
        other => bail!("unknown action {other:?}; use screenshot, click, move, drag, type, key, scroll, or wait"),
    }
    Ok(true)
}

/// Hold all but the last key of a combination, press the last, and let go in reverse order,
/// also when a press fails halfway.
fn press(enigo: &mut Enigo, keys: &[Key]) -> Result<()> {
    let Some((last, held)) = keys.split_last() else {
        return Ok(());
    };
    let mut down = 0;
    let mut done = Ok(());
    for key in held {
        done = enigo.key(*key, Direction::Press);
        if done.is_err() {
            break;
        }
        down += 1;
    }
    if done.is_ok() {
        done = enigo.key(*last, Direction::Click);
    }
    for key in held[..down].iter().rev() {
        let released = enigo.key(*key, Direction::Release);
        done = done.and(released);
    }
    done.map_err(fail("the key press"))
}

/// The keys of a combination like `ctrl+shift+t`, named as xdotool (and the container's driver)
/// names them, plus the names a Mac's keyboard has.
fn combo(name: &str) -> Result<Vec<Key>> {
    name.split('+').map(key).collect::<Option<Vec<_>>>().ok_or_else(|| {
        anyhow!("unknown key {name:?}; use names like Return, Escape, Tab, BackSpace, Delete, Page_Down, F5, ctrl+s, cmd+c or alt+F4")
    })
}

fn key(name: &str) -> Option<Key> {
    let lower = name.to_ascii_lowercase();
    Some(match lower.as_str() {
        "ctrl" | "control" => Key::Control,
        "alt" | "option" | "opt" => Key::Alt,
        "shift" => Key::Shift,
        "super" | "cmd" | "command" | "meta" | "win" | "windows" => Key::Meta,
        "return" | "enter" | "kp_enter" => Key::Return,
        "escape" | "esc" => Key::Escape,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "delete" | "del" => Key::Delete,
        "space" => Key::Space,
        "up" => Key::UpArrow,
        "down" => Key::DownArrow,
        "left" => Key::LeftArrow,
        "right" => Key::RightArrow,
        "home" => Key::Home,
        "end" => Key::End,
        "page_up" | "pageup" | "prior" => Key::PageUp,
        "page_down" | "pagedown" | "next" => Key::PageDown,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        "minus" => Key::Unicode('-'),
        "plus" => Key::Unicode('+'),
        "comma" => Key::Unicode(','),
        "period" => Key::Unicode('.'),
        "slash" => Key::Unicode('/'),
        _ => {
            // A single character is itself: `a`, `7`, `ä`. In a combination it is the key, so
            // lower case (`ctrl+S` is ctrl+s; shift is named when it is meant).
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => Key::Unicode(c.to_lowercase().next().unwrap_or(c)),
                _ => return None,
            }
        }
    })
}

/// The main display as JPEG bytes, scaled to `width` x `height`.
fn screenshot(width: u32, height: u32) -> Result<Vec<u8>> {
    let monitors = xcap::Monitor::all().map_err(|e| anyhow!("the screen can't be captured: {e}"))?;
    let main = monitors
        .iter()
        .find(|m| m.is_primary().unwrap_or(false))
        .or(monitors.first())
        .ok_or_else(|| anyhow!("no screen was found"))?;
    let shot = main
        .capture_image()
        .map_err(|e| anyhow!("the screen can't be captured: {e}"))?;
    // xcap's `image` may be another version of the crate than cedar's, so the pixels cross over
    // as plain bytes.
    let (w, h) = (shot.width(), shot.height());
    let rgba = image::RgbaImage::from_raw(w, h, shot.into_raw())
        .ok_or_else(|| anyhow!("the screenshot is malformed"))?;
    let mut image = DynamicImage::ImageRgba8(rgba);
    if (w, h) != (width, height) {
        image = image.resize_exact(width, height, FilterType::CatmullRom);
    }
    let mut out = Vec::new();
    DynamicImage::ImageRgb8(image.to_rgb8())
        .write_with_encoder(JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY))
        .context("encoding the screenshot")?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combinations_are_read_as_xdotool_names_them() {
        assert_eq!(combo("ctrl+s").unwrap(), [Key::Control, Key::Unicode('s')]);
        assert_eq!(combo("ctrl+S").unwrap(), [Key::Control, Key::Unicode('s')]);
        assert_eq!(
            combo("ctrl+shift+t").unwrap(),
            [Key::Control, Key::Shift, Key::Unicode('t')]
        );
        assert_eq!(combo("cmd+c").unwrap(), [Key::Meta, Key::Unicode('c')]);
        assert_eq!(combo("alt+F4").unwrap(), [Key::Alt, Key::F4]);
        assert_eq!(combo("Return").unwrap(), [Key::Return]);
        assert_eq!(combo("BackSpace").unwrap(), [Key::Backspace]);
        assert_eq!(combo("Page_Down").unwrap(), [Key::PageDown]);
        assert_eq!(combo("ä").unwrap(), [Key::Unicode('ä')]);
        for bad in ["ctrl+", "hyper+x", "Returnn", ""] {
            assert!(combo(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn positions_must_be_on_the_screen() {
        let screen = (1440, 900);
        assert_eq!(point(&json!({ "x": 0, "y": 899 }), screen, "x", "y").unwrap(), (0, 899));
        for bad in [json!({ "x": 1440, "y": 1 }), json!({ "x": -1, "y": 1 }), json!({ "x": 1 })] {
            assert!(point(&bad, screen, "x", "y").is_err(), "{bad}");
        }
    }
}
