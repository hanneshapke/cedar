//! The engine of the contribution site (`contrib-web/web`): a person drops their ChatGPT or
//! Claude data export on the page, picks one conversation, reviews it and sends it to the corpus
//! cedar-chat's conversations go to (`contrib/`). This crate turns that conversation into a
//! bundle (`cedar-contrib-schema`), with the same redaction scan and consistency checks the
//! ingest runs, compiled to WebAssembly so the export never leaves the browser: the only thing
//! that does is the bundle, when the person presses Send.
//!
//! The WebAssembly interface is three functions and JSON strings, no bindings generator:
//! `cw_alloc(len) -> ptr`, `cw_free(ptr, len)`, and `cw_build(ptr, len) -> ptr`, which reads a
//! request (see [`Request`]) and returns a pointer to a 4-byte little-endian length followed by
//! that many bytes of JSON: `{"ok": Preview}` or `{"error": "..."}`. The caller frees the result
//! with `cw_free(ptr, 4 + length)`.

pub mod build;
pub mod chatgpt;
pub mod claude;
pub mod transcript;

use serde::Deserialize;
use serde_json::{json, Value};

/// What the page asks for: one conversation as the export has it, the review's choices, and the
/// seed for the stand-ins (random per conversation, kept while the review is open so they stay
/// put as the person changes options).
#[derive(Debug, Deserialize)]
pub struct Request {
    pub conversation: Value,
    /// 16 hex digits.
    pub seed: String,
    #[serde(default)]
    pub options: build::Options,
}

/// Handle one request, JSON in, JSON out.
pub fn handle(input: &str) -> String {
    let result = (|| -> Result<build::Preview, String> {
        let req: Request = serde_json::from_str(input).map_err(|e| format!("bad request: {e}"))?;
        let seed = u64::from_str_radix(req.seed.trim(), 16).map_err(|_| "seed must be 16 hex digits".to_string())?;
        let transcript = match build::detect(&req.conversation) {
            Some(transcript::Source::ChatGpt) => chatgpt::import(&req.conversation),
            Some(transcript::Source::Claude) => claude::import(&req.conversation),
            None => return Err("this is neither a ChatGPT nor a Claude conversation".into()),
        };
        Ok(build::build(&transcript, &req.options, seed))
    })();
    match result {
        Ok(p) => json!({ "ok": p }).to_string(),
        Err(e) => json!({ "error": e }).to_string(),
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm {
    /// Memory for the caller to write a request into.
    #[no_mangle]
    pub extern "C" fn cw_alloc(len: usize) -> *mut u8 {
        let mut buf = std::mem::ManuallyDrop::new(vec![0u8; len].into_boxed_slice());
        buf.as_mut_ptr()
    }

    /// Give back what `cw_alloc` or `cw_build` handed out.
    ///
    /// # Safety
    /// `ptr` and `len` must be exactly what one of them returned (for `cw_build`'s result, the
    /// length prefix plus 4), and freed once.
    #[no_mangle]
    pub unsafe extern "C" fn cw_free(ptr: *mut u8, len: usize) {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
    }

    /// # Safety
    /// `ptr..ptr+len` must be memory from `cw_alloc` holding UTF-8; it is freed here.
    #[no_mangle]
    pub unsafe extern "C" fn cw_build(ptr: *mut u8, len: usize) -> *mut u8 {
        let input = Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len));
        let out = match std::str::from_utf8(&input) {
            Ok(s) => super::handle(s),
            Err(_) => r#"{"error":"the request is not UTF-8"}"#.to_string(),
        };
        drop(input);
        let mut buf = Vec::with_capacity(out.len() + 4);
        buf.extend_from_slice(&(out.len() as u32).to_le_bytes());
        buf.extend_from_slice(out.as_bytes());
        let mut buf = std::mem::ManuallyDrop::new(buf.into_boxed_slice());
        buf.as_mut_ptr()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_errors_are_json() {
        let conv = chatgpt::fixture("t", &[("user", "", json!({ "content_type": "text", "parts": ["hi there, how are you"] })), ("assistant", "all", json!({ "content_type": "text", "parts": ["Fine."] }))]);
        let out: Value = serde_json::from_str(&handle(&json!({ "conversation": conv, "seed": "00000000000000ff" }).to_string())).unwrap();
        assert_eq!(out["ok"]["bundle"]["turns"].as_array().unwrap().len(), 1);
        assert!(out["ok"]["json"].is_string());
        let out: Value = serde_json::from_str(&handle(&json!({ "conversation": {}, "seed": "1" }).to_string())).unwrap();
        assert!(out["error"].as_str().unwrap().contains("neither"));
        let out: Value = serde_json::from_str(&handle("nope")).unwrap();
        assert!(out["error"].is_string());
    }
}
