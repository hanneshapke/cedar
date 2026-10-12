//! Tools the model can call. Each one runs inside the run's sandbox.

mod bash;
mod files;

pub use bash::Bash;
pub use files::{ReadFile, WriteFile};

use crate::sandbox::Sandbox;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// Everything a tool needs to do its work for one run.
pub struct ToolContext {
    pub sandbox: Arc<dyn Sandbox>,
    /// Per-command wall-clock limit.
    pub timeout: Duration,
    /// Cap on returned output, per stream.
    pub max_output: usize,
}

/// A tool is a name, a description, a JSON schema for its input, and a function.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> Value;
    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String>;

    /// This tool's entry in the `tools` array handed to the LLM client.
    fn definition(&self) -> Value {
        json!({
            "name": self.name(),
            "description": self.description(),
            "input_schema": self.schema(),
        })
    }
}

/// The tools available to every run.
pub struct ToolSet {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolSet {
    pub fn new(tools: Vec<Box<dyn Tool>>) -> Self {
        Self { tools }
    }

    /// `bash`, `read_file`, and `write_file`.
    pub fn standard() -> Self {
        Self::new(vec![
            Box::new(Bash),
            Box::new(ReadFile),
            Box::new(WriteFile),
        ])
    }

    pub fn definitions(&self) -> Vec<Value> {
        self.tools.iter().map(|t| t.definition()).collect()
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|t| t.as_ref())
    }
}

/// Cut `s` to at most `max` bytes (on a char boundary), noting how much was dropped.
pub fn truncate_output(s: &str, max: usize) -> String {
    truncate_with_total(s, s.len(), max)
}

/// Like [`truncate_output`] when `s` is itself already a prefix of an output `total` bytes long.
pub fn truncate_with_total(s: &str, total: usize, max: usize) -> String {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    // `s` may be longer than `total` if invalid UTF-8 was replaced by U+FFFD, hence the saturation.
    if end == s.len() && total <= end {
        return s.to_string();
    }
    format!(
        "{}\n[truncated {} bytes]",
        &s[..end],
        total.saturating_sub(end)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_output_is_untouched() {
        assert_eq!(truncate_output("hello", 10), "hello");
        assert_eq!(truncate_output("hello", 5), "hello");
    }

    #[test]
    fn long_output_is_cut_and_marked() {
        assert_eq!(
            truncate_output("hello world", 5),
            "hello\n[truncated 6 bytes]"
        );
    }

    #[test]
    fn truncation_respects_char_boundaries() {
        // "é" is two bytes; cutting at 1 must back off to 0 instead of panicking.
        assert_eq!(truncate_output("é!", 1), "\n[truncated 3 bytes]");
        assert_eq!(truncate_output("aé", 2), "a\n[truncated 2 bytes]");
    }

    #[test]
    fn lossy_expansion_does_not_underflow() {
        // One invalid byte becomes a 3-byte U+FFFD, so the string is longer than the raw total.
        let s = String::from_utf8_lossy(&[0xff]).into_owned();
        assert_eq!(truncate_with_total(&s, 1, 10), s);
        assert_eq!(truncate_with_total(&s, 1, 1), "\n[truncated 1 bytes]");
    }

    #[test]
    fn prefix_with_larger_total_reports_the_remainder() {
        assert_eq!(
            truncate_with_total("abc", 1000, 3),
            "abc\n[truncated 997 bytes]"
        );
    }

    #[test]
    fn standard_set_exposes_the_local_tools() {
        let set = ToolSet::standard();
        let defs = set.definitions();
        let names: Vec<_> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["bash", "read_file", "write_file"]);
        assert!(set.get("bash").is_some());
        assert!(set.get("nope").is_none());
    }
}
