//! `computer`: work a desktop by looking at its screen and using its mouse and keyboard.
//!
//! The desktop runs in its own container for the run (see [`crate::computer`]); this tool checks
//! the model's request, passes it on, and returns the screenshot the driver took afterwards.
//! Positions are the model's, in the screenshot it was shown: when the server's `max_image_side`
//! is smaller than the screen, the screenshot is scaled down and positions are scaled back up
//! here, so the model never has to know.
//!
//! With `COMPUTER_RUNTIME=native` the desktop is the user's own computer. The tool then says so,
//! has no `open` (the workspace is not on that computer), and takes the size of the screen from
//! the computer itself.

use super::{Tool, ToolContext, ToolImage, ToolOutput};
use crate::config::ComputerConfig;
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use serde_json::{json, Map, Value};
use std::time::Duration;

/// How long one call may take. Longer than anything the driver waits for itself (a ten second
/// `wait`, a long text typed out), so a timeout means the desktop is stuck.
const CALL_TIMEOUT: Duration = Duration::from_secs(90);
const ACTIONS: &str = "screenshot, click, move, drag, type, key, scroll, wait, or open";
const NATIVE_ACTIONS: &str = "screenshot, click, move, drag, type, key, scroll, or wait";

pub struct ComputerTool {
    /// The user's own computer rather than a desktop in a container.
    native: bool,
    /// The size of a container's screen in pixels; the user's own says its size when asked.
    screen: Option<(u32, u32)>,
    description: String,
}

const NATIVE_DESCRIPTION: &str = "Work the user's own computer, the one they are sitting at, by \
     looking at its screen and using its mouse and keyboard: for something in an application \
     on their computer that no other tool can reach. This is their real computer with their \
     real files, accounts and open windows, not a sandbox: what you do there can't be taken \
     back, and the user is asked before your calls run. Do only what the user asked for. Don't \
     close or change what they have open beyond the task, and before anything that sends, \
     buys, deletes or overwrites, say what you are about to do and ask them first. If the \
     screen shows something unrelated and private, leave it alone and don't repeat it. Every \
     call returns a screenshot of the screen after the action. Positions are pixels in the \
     latest screenshot, x from its left edge and y from its top. Look before you act: start \
     with action \"screenshot\", aim at the middle of what you want to click, and check each \
     screenshot to see whether the action did what you meant before going on. Your other tools \
     (bash, the file tools) work in a separate sandbox and can't see this computer's files.";

impl ComputerTool {
    pub fn new(cfg: &ComputerConfig) -> Self {
        if cfg.native() {
            return Self { native: true, screen: None, description: NATIVE_DESCRIPTION.to_string() };
        }
        let net = if cfg.online() {
            "The desktop has internet access through its web browser."
        } else {
            "The desktop has no internet access."
        };
        Self {
            native: false,
            screen: Some((cfg.width, cfg.height)),
            description: format!(
                "Work a desktop computer by looking at its screen and using its mouse and \
                 keyboard: for applications with windows (a spreadsheet or document in an \
                 office suite, a web browser, a file manager), when no other tool can do the \
                 job. Every call returns a screenshot of the screen after the action. Positions \
                 are pixels in the latest screenshot, x from its left edge and y from its top. \
                 Look before you act: start with action \"screenshot\", aim at the middle of \
                 what you want to click, and check each screenshot to see whether the action \
                 did what you meant before going on. The desktop keeps its windows between \
                 calls. It shares the workspace with your other tools, at /workspace: open a \
                 file there with action \"open\", and save what you want to keep there. {net}"
            ),
        }
    }
}

#[async_trait]
impl Tool for ComputerTool {
    fn name(&self) -> &str {
        "computer"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn schema(&self) -> Value {
        let mut schema = self.full_schema();
        if self.native {
            // No workspace on the user's computer to open a file from.
            if let Some(properties) = schema["properties"].as_object_mut() {
                properties.remove("path");
            }
            let action = &mut schema["properties"]["action"];
            if let Some(actions) = action["enum"].as_array_mut() {
                actions.retain(|a| a != "open");
            }
            let described = action["description"].as_str().unwrap_or_default();
            let cut = described.find(" open:").unwrap_or(described.len());
            action["description"] = json!(format!(
                "{} key: also cmd+c and the like on a Mac.",
                &described[..cut]
            ));
        }
        schema
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        Ok(self.call_output(ctx, input).await?.text)
    }

    async fn call_output(&self, ctx: &ToolContext, input: &Value) -> Result<ToolOutput> {
        let session = ctx
            .computer
            .as_ref()
            .ok_or_else(|| anyhow!("the computer is not available"))?;
        if self.native && input["action"] == "open" {
            bail!("unknown action \"open\"; use {NATIVE_ACTIONS}");
        }
        let screen = match self.screen {
            Some(screen) => screen,
            None => session.screen().await?,
        };
        let shown = shown_size(screen, ctx.max_image_side);
        let request = request(input, screen, shown)?;
        let reply = session.call(&request, CALL_TIMEOUT).await?;
        output(&reply, screen, shown, self.native)
    }
}

impl ComputerTool {
    fn full_schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "action": { "type": "string",
                                "enum": ["screenshot", "click", "move", "drag", "type", "key", "scroll", "wait", "open"],
                                "description": "screenshot: only look. click: click at `x`,`y`. move: move the pointer to `x`,`y`. drag: press the left button at `x`,`y` and release it at `to_x`,`to_y`. type: type `text` into whatever has the keyboard (click the field first). key: press `keys`. scroll: turn the mouse wheel, at `x`,`y` if given. wait: wait `seconds` for the screen to catch up. open: open the workspace file `path` with its application." },
                    "x": { "type": "integer", "minimum": 0, "description": "click, move, drag, scroll: pixels from the left edge of the screenshot." },
                    "y": { "type": "integer", "minimum": 0, "description": "click, move, drag, scroll: pixels from the top of the screenshot." },
                    "to_x": { "type": "integer", "minimum": 0, "description": "drag: where to release, from the left edge." },
                    "to_y": { "type": "integer", "minimum": 0, "description": "drag: where to release, from the top." },
                    "button": { "type": "string", "enum": ["left", "right", "middle"], "description": "click: default left." },
                    "count": { "type": "integer", "minimum": 1, "maximum": 3, "description": "click: 2 for a double click, 3 for a triple click (default 1)." },
                    "text": { "type": "string", "description": "type: the text; a newline presses Enter." },
                    "keys": { "type": "string", "description": "key: a key or combination by its X name, like Return, Escape, Tab, BackSpace, Delete, Page_Down, ctrl+s, ctrl+shift+t or alt+F4; several are pressed in turn when separated by spaces." },
                    "direction": { "type": "string", "enum": ["down", "up", "left", "right"], "description": "scroll: default down." },
                    "amount": { "type": "integer", "minimum": 1, "maximum": 30, "description": "scroll: notches of the wheel (default 3)." },
                    "seconds": { "type": "number", "minimum": 0, "maximum": 10, "description": "wait: default 1." },
                    "path": { "type": "string", "description": "open: the file, relative to the workspace." }
                },
                "required": ["action"] })
    }

}

/// The size the screenshot is shown at: the screen's, scaled down to fit `max_side`.
fn shown_size(screen: (u32, u32), max_side: u32) -> (u32, u32) {
    let longer = screen.0.max(screen.1);
    if longer <= max_side {
        return screen;
    }
    let scale = |side: u32| ((u64::from(side) * u64::from(max_side)) / u64::from(longer)).max(16) as u32;
    (scale(screen.0), scale(screen.1))
}

/// A position in a picture `from` pixels long as the middle of the same pixel in one `to` long.
fn rescale(at: u64, from: u32, to: u32) -> u64 {
    if from == to {
        return at;
    }
    ((2 * at + 1) * u64::from(to) / (2 * u64::from(from))).min(u64::from(to) - 1)
}

/// The request for the driver, holding only what `input`'s action takes, with positions turned
/// from the screenshot's pixels into the screen's.
fn request(input: &Value, screen: (u32, u32), shown: (u32, u32)) -> Result<Value> {
    let action = input["action"]
        .as_str()
        .ok_or_else(|| anyhow!("missing action"))?;
    let mut req = Map::new();
    req.insert("action".into(), action.into());
    let coordinate = |name: &str, shown: u32, screen: u32| -> Result<Value> {
        let at = input[name].as_u64().filter(|at| *at < u64::from(shown)).ok_or_else(|| {
            anyhow!("{action} needs `{name}`, a whole number from 0 to {} (the screenshot is {shown} pixels along it)", shown - 1)
        })?;
        Ok(rescale(at, shown, screen).into())
    };
    let point = |req: &mut Map<String, Value>, x: &str, y: &str| -> Result<()> {
        req.insert(x.into(), coordinate(x, shown.0, screen.0)?);
        req.insert(y.into(), coordinate(y, shown.1, screen.1)?);
        Ok(())
    };
    let within = |name: &str, low: u64, high: u64, default: u64| -> Result<u64> {
        match &input[name] {
            Value::Null => Ok(default),
            v => v
                .as_u64()
                .filter(|n| (low..=high).contains(n))
                .ok_or_else(|| anyhow!("`{name}` must be a whole number from {low} to {high}")),
        }
    };
    let one_of = |name: &str, choices: &[&str]| -> Result<String> {
        match &input[name] {
            Value::Null => Ok(choices[0].to_string()),
            v => v
                .as_str()
                .filter(|c| choices.contains(c))
                .map(String::from)
                .ok_or_else(|| anyhow!("`{name}` must be one of: {}", choices.join(", "))),
        }
    };
    match action {
        "screenshot" => {}
        "click" => {
            point(&mut req, "x", "y")?;
            req.insert("button".into(), one_of("button", &["left", "right", "middle"])?.into());
            req.insert("count".into(), within("count", 1, 3, 1)?.into());
        }
        "move" => point(&mut req, "x", "y")?,
        "drag" => {
            point(&mut req, "x", "y")?;
            point(&mut req, "to_x", "to_y")?;
        }
        "type" => {
            let text = input["text"]
                .as_str()
                .filter(|t| !t.is_empty())
                .ok_or_else(|| anyhow!("type needs `text`"))?;
            req.insert("text".into(), text.into());
        }
        "key" => {
            let keys = input["keys"]
                .as_str()
                .map(str::trim)
                .filter(|k| !k.is_empty())
                .ok_or_else(|| anyhow!("key needs `keys`, like Return or ctrl+s"))?;
            req.insert("keys".into(), keys.into());
        }
        "scroll" => {
            if !input["x"].is_null() || !input["y"].is_null() {
                point(&mut req, "x", "y")?;
            }
            let direction = one_of("direction", &["down", "up", "left", "right"])?;
            req.insert("direction".into(), direction.into());
            req.insert("amount".into(), within("amount", 1, 30, 3)?.into());
        }
        "wait" => {
            let seconds = match &input["seconds"] {
                Value::Null => 1.0,
                v => v
                    .as_f64()
                    .filter(|s| (0.0..=10.0).contains(s))
                    .ok_or_else(|| anyhow!("`seconds` must be a number from 0 to 10"))?,
            };
            req.insert("seconds".into(), seconds.into());
        }
        "open" => {
            let path = input["path"]
                .as_str()
                .map(|p| p.trim().trim_start_matches("/workspace/"))
                .filter(|p| !p.is_empty())
                .ok_or_else(|| anyhow!("open needs `path`, a file in the workspace"))?;
            crate::sandbox::check_workspace_path(path)?;
            req.insert("path".into(), path.into());
        }
        other => bail!("unknown action {other:?}; use {ACTIONS}"),
    }
    if shown != screen {
        req.insert("width".into(), shown.0.into());
        req.insert("height".into(), shown.1.into());
    }
    Ok(Value::Object(req))
}

/// The tool result: the screenshot, and in words what a screenshot doesn't say outright.
fn output(reply: &Value, screen: (u32, u32), shown: (u32, u32), native: bool) -> Result<ToolOutput> {
    let data = reply["image"]
        .as_str()
        .filter(|d| !d.is_empty())
        .ok_or_else(|| anyhow!("the computer sent no screenshot"))?;
    let media_type = reply["media_type"].as_str().unwrap_or("image/png");
    let mut text = format!(
        "The screen after the action is in the screenshot ({}x{} pixels).",
        shown.0, shown.1
    );
    if let (Some(x), Some(y)) = (reply["pointer"][0].as_u64(), reply["pointer"][1].as_u64()) {
        text.push_str(&format!(
            " The pointer is at ({}, {}).",
            rescale(x.min(u64::from(screen.0) - 1), screen.0, shown.0),
            rescale(y.min(u64::from(screen.1) - 1), screen.1, shown.1)
        ));
    }
    match reply["window"].as_str().filter(|w| !w.trim().is_empty()) {
        Some(window) => {
            let title: String = window.chars().take(120).collect();
            text.push_str(&format!(" The window with the keyboard is {title:?}."));
        }
        // The user's own computer doesn't say which window that is.
        None if native => {}
        None => text.push_str(" No window has the keyboard."),
    }
    Ok(ToolOutput {
        text,
        image: Some(ToolImage {
            media_type: media_type.to_string(),
            data: data.to_string(),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: (u32, u32) = (1280, 800);

    fn req(input: Value) -> Value {
        request(&input, SCREEN, SCREEN).unwrap()
    }

    #[test]
    fn requests_carry_only_what_their_action_takes() {
        assert_eq!(
            req(json!({ "action": "screenshot", "x": 3, "text": "x" })),
            json!({ "action": "screenshot" })
        );
        assert_eq!(
            req(json!({ "action": "click", "x": 640, "y": 400 })),
            json!({ "action": "click", "x": 640, "y": 400, "button": "left", "count": 1 })
        );
        assert_eq!(
            req(json!({ "action": "click", "x": 0, "y": 799, "button": "right", "count": 2 })),
            json!({ "action": "click", "x": 0, "y": 799, "button": "right", "count": 2 })
        );
        assert_eq!(
            req(json!({ "action": "drag", "x": 1, "y": 2, "to_x": 3, "to_y": 4 })),
            json!({ "action": "drag", "x": 1, "y": 2, "to_x": 3, "to_y": 4 })
        );
        assert_eq!(
            req(json!({ "action": "type", "text": "Hood River\n" })),
            json!({ "action": "type", "text": "Hood River\n" })
        );
        assert_eq!(
            req(json!({ "action": "key", "keys": " ctrl+s " })),
            json!({ "action": "key", "keys": "ctrl+s" })
        );
        assert_eq!(
            req(json!({ "action": "scroll" })),
            json!({ "action": "scroll", "direction": "down", "amount": 3 })
        );
        assert_eq!(
            req(json!({ "action": "scroll", "x": 5, "y": 6, "direction": "up", "amount": 10 })),
            json!({ "action": "scroll", "x": 5, "y": 6, "direction": "up", "amount": 10 })
        );
        assert_eq!(req(json!({ "action": "wait" })), json!({ "action": "wait", "seconds": 1.0 }));
        assert_eq!(
            req(json!({ "action": "open", "path": "/workspace/out/report.ods" })),
            json!({ "action": "open", "path": "out/report.ods" })
        );
    }

    #[test]
    fn bad_requests_are_explained() {
        for (input, expected) in [
            (json!({}), "missing action"),
            (json!({ "action": "evaluate" }), "unknown action"),
            (json!({ "action": "click" }), "needs `x`"),
            (json!({ "action": "click", "x": 10 }), "needs `y`"),
            (json!({ "action": "click", "x": 1280, "y": 10 }), "from 0 to 1279"),
            (json!({ "action": "click", "x": 10, "y": -1 }), "needs `y`"),
            (json!({ "action": "click", "x": "10", "y": 1 }), "needs `x`"),
            (json!({ "action": "click", "x": 1, "y": 1, "button": "back" }), "`button`"),
            (json!({ "action": "click", "x": 1, "y": 1, "count": 4 }), "`count`"),
            (json!({ "action": "drag", "x": 1, "y": 1 }), "needs `to_x`"),
            (json!({ "action": "type" }), "needs `text`"),
            (json!({ "action": "key", "keys": " " }), "needs `keys`"),
            (json!({ "action": "scroll", "direction": "sideways" }), "`direction`"),
            (json!({ "action": "scroll", "x": 5 }), "needs `y`"),
            (json!({ "action": "wait", "seconds": 60 }), "`seconds`"),
            (json!({ "action": "open" }), "needs `path`"),
            (json!({ "action": "open", "path": "../etc/passwd" }), "`..`"),
            (json!({ "action": "open", "path": "/etc/passwd" }), "relative"),
        ] {
            let err = request(&input, SCREEN, SCREEN).unwrap_err().to_string();
            assert!(err.contains(expected), "{input}: {err}");
        }
    }

    #[test]
    fn a_screenshot_scaled_down_keeps_positions_right() {
        assert_eq!(shown_size(SCREEN, 1568), SCREEN);
        assert_eq!(shown_size(SCREEN, 1280), SCREEN);
        let shown = shown_size(SCREEN, 640);
        assert_eq!(shown, (640, 400));

        // The model points at the screenshot; the driver is told where that is on the screen
        // and how large a screenshot to send back.
        let r = request(&json!({ "action": "click", "x": 320, "y": 200 }), SCREEN, shown).unwrap();
        assert_eq!((r["x"].as_u64(), r["y"].as_u64()), (Some(641), Some(401)));
        assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(640), Some(400)));
        let corner = request(&json!({ "action": "move", "x": 639, "y": 399 }), SCREEN, shown).unwrap();
        assert_eq!((corner["x"].as_u64(), corner["y"].as_u64()), (Some(1279), Some(799)));
        let err = request(&json!({ "action": "move", "x": 640, "y": 0 }), SCREEN, shown).unwrap_err();
        assert!(err.to_string().contains("from 0 to 639"), "{err}");

        // And the pointer is reported where the model sees it.
        let reply = json!({ "image": "aGk=", "pointer": [641, 401], "window": null });
        let out = output(&reply, SCREEN, shown, false).unwrap();
        assert!(out.text.contains("(640x400 pixels)"), "{}", out.text);
        assert!(out.text.contains("The pointer is at (320, 200)."), "{}", out.text);
    }

    #[test]
    fn the_result_is_the_screenshot_with_what_it_does_not_say() {
        let reply = json!({
            "image": "aGk=", "media_type": "image/png", "width": 1280, "height": 800,
            "screen": [1280, 800], "pointer": [640, 400], "window": "report.ods - LibreOffice Calc",
        });
        let out = output(&reply, SCREEN, SCREEN, false).unwrap();
        assert_eq!(
            out.text,
            "The screen after the action is in the screenshot (1280x800 pixels). The pointer is \
             at (640, 400). The window with the keyboard is \"report.ods - LibreOffice Calc\"."
        );
        assert_eq!(
            out.image,
            Some(ToolImage { media_type: "image/png".into(), data: "aGk=".into() })
        );

        let bare = output(&json!({ "image": "aGk=" }), SCREEN, SCREEN, false).unwrap();
        assert!(bare.text.ends_with("No window has the keyboard."), "{}", bare.text);
        let err = output(&json!({ "pointer": [1, 1] }), SCREEN, SCREEN, false).unwrap_err();
        assert_eq!(err.to_string(), "the computer sent no screenshot");
    }

    #[test]
    fn the_description_says_whether_the_desktop_is_online() {
        let mut cfg = ComputerConfig {
            enabled: true,
            runtime: crate::config::ComputerRuntime::Docker,
            docker_bin: "docker".into(),
            image: "computer:1".into(),
            network: "bridge".into(),
            memory: "2g".into(),
            cpus: "2".into(),
            width: 1280,
            height: 800,
        };
        assert!(ComputerTool::new(&cfg).description().contains("has internet access"));
        cfg.network = "none".into();
        assert!(ComputerTool::new(&cfg).description().contains("has no internet access"));

        // The user's own computer is described as what it is, and has no workspace to open from.
        let docker = ComputerTool::new(&cfg).schema();
        assert!(docker["properties"]["action"]["enum"].as_array().unwrap().contains(&json!("open")));
        cfg.runtime = crate::config::ComputerRuntime::Native;
        let native = ComputerTool::new(&cfg);
        assert!(native.description().contains("their real computer"), "{}", native.description());
        let schema = native.schema();
        let action = &schema["properties"]["action"];
        assert!(!action["enum"].as_array().unwrap().contains(&json!("open")));
        assert!(!action["description"].as_str().unwrap().contains("open:"), "{action}");
        assert!(schema["properties"].get("path").is_none());
        let bare = output(&json!({ "image": "aGk=" }), SCREEN, SCREEN, true).unwrap();
        assert!(bare.text.ends_with("pixels)."), "{}", bare.text);
    }
}
