//! Tools the model can call. The file, shell, and PDF tools run inside the run's sandbox;
//! `web_search` runs in the cedar process, because the sandbox has no network.

mod bash;
mod files;
mod pdf;
mod web_search;

pub use bash::Bash;
pub use files::{ReadFile, WriteFile};
pub use pdf::GeneratePdf;
pub use web_search::BraveSearch;

use crate::config::Config;
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

    /// `bash`, `read_file`, `write_file`, and `generate_pdf`.
    pub fn standard() -> Self {
        Self::new(vec![
            Box::new(Bash),
            Box::new(ReadFile),
            Box::new(WriteFile),
            Box::new(GeneratePdf),
        ])
    }

    /// The standard tools, plus `web_search` if a Brave API key is configured.
    pub fn from_config(cfg: &Config) -> Result<Self> {
        let mut set = Self::standard();
        if let Some(key) = &cfg.brave_api_key {
            set.tools.push(Box::new(BraveSearch::new(
                key.clone(),
                cfg.brave_base_url.clone(),
            )?));
        }
        Ok(set)
    }

    /// A note for the system prompt saying what the sandbox can reach and how to search the web,
    /// so the model doesn't try to fetch search engines with `curl`.
    pub fn environment_note(&self, sandbox_network: &str) -> String {
        let search = self.get("web_search").is_some();
        let net = if sandbox_network == "none" {
            "The sandbox has no internet access: curl, wget, and pip install cannot reach the network."
        } else {
            "The sandbox has outbound network access (e.g. for pip install)."
        };
        let how = if search {
            "To look something up on the web, use the web_search tool; do not query search engines \
             with curl or wget."
        } else {
            "No web search tool is available, and search engines block scripted requests, so do not \
             query them with curl or wget. If a task needs information from the web, say so."
        };
        format!("{net} {how}")
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
        assert_eq!(names, ["bash", "read_file", "write_file", "generate_pdf"]);
        assert!(set.get("bash").is_some());
        assert!(set.get("nope").is_none());
    }

    #[test]
    fn environment_note_points_to_web_search_when_offered() {
        let cfg = |pairs: &[(&str, &str)]| {
            let pairs: Vec<_> = pairs.to_vec();
            Config::from_lookup(move |k| {
                pairs
                    .iter()
                    .find(|(name, _)| *name == k)
                    .map(|(_, v)| v.to_string())
            })
            .unwrap()
        };
        let plain = ToolSet::from_config(&cfg(&[("CEDAR_API_KEYS", "a")])).unwrap();
        let search =
            ToolSet::from_config(&cfg(&[("CEDAR_API_KEYS", "a"), ("BRAVE_API_KEY", "k")])).unwrap();

        let note = search.environment_note("none");
        assert!(note.contains("no internet access"));
        assert!(note.contains("use the web_search tool"));
        let note = plain.environment_note("none");
        assert!(note.contains("No web search tool is available"));
        assert!(!note.contains("use the web_search tool"));
        assert!(plain
            .environment_note("bridge")
            .contains("outbound network access"));
    }

    fn tool_names(cfg: &Config) -> Vec<String> {
        ToolSet::from_config(cfg)
            .unwrap()
            .definitions()
            .iter()
            .map(|d| d["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn web_search_is_offered_only_with_a_brave_key() {
        let cfg = |pairs: &[(&str, &str)]| {
            let pairs: Vec<_> = pairs.to_vec();
            Config::from_lookup(move |k| {
                pairs
                    .iter()
                    .find(|(name, _)| *name == k)
                    .map(|(_, v)| v.to_string())
            })
            .unwrap()
        };
        assert_eq!(
            tool_names(&cfg(&[("CEDAR_API_KEYS", "a")])),
            ["bash", "read_file", "write_file", "generate_pdf"]
        );
        assert_eq!(
            tool_names(&cfg(&[("CEDAR_API_KEYS", "a"), ("BRAVE_API_KEY", "k")])),
            [
                "bash",
                "read_file",
                "write_file",
                "generate_pdf",
                "web_search"
            ]
        );
    }
}
