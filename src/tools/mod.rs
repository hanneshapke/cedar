//! Tools the model can call. The file, shell, Python, document, and chart tools run inside the
//! run's sandbox; `web_search`, `fetch_url`, `download_file`, `weather_forecast`, and `show_map` run
//! in the cedar process, because the sandbox has no network; `browser` drives the run's browser
//! container.

mod bash;
mod browser;
mod chart;
mod document;
mod fetch;
mod files;
mod map;
mod open_meteo;
mod pdf;
mod python;
mod weather;
mod web_search;

pub use bash::Bash;
pub use browser::BrowserTool;
pub use chart::GenerateChart;
pub use document::ReadDocument;
pub use fetch::{DownloadFile, FetchUrl, WebClient};
pub use files::{ReadFile, WriteFile};
pub use map::ShowMap;
pub use pdf::GeneratePdf;
pub use python::RunPython;
pub use weather::WeatherForecast;
pub use web_search::BraveSearch;

use crate::browser::BrowserSession;
use crate::config::Config;
use crate::sandbox::{ExecOutput, Sandbox};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// Everything a tool needs to do its work for one run.
pub struct ToolContext {
    pub sandbox: Arc<dyn Sandbox>,
    /// The run's browser, started on first use. `None` when the browser is turned off.
    pub browser: Option<Arc<BrowserSession>>,
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

    /// The tools that run in the sandbox: `bash`, `read_file`, `write_file`, `run_python`,
    /// `read_document`, `generate_pdf`, and `generate_chart`.
    pub fn standard() -> Self {
        Self::new(vec![
            Box::new(Bash),
            Box::new(ReadFile),
            Box::new(WriteFile),
            Box::new(RunPython),
            Box::new(ReadDocument),
            Box::new(GeneratePdf),
            Box::new(GenerateChart),
        ])
    }

    /// The standard tools, plus `web_search` if a Brave API key is configured, `fetch_url` and
    /// `download_file` unless `WEB_FETCH` is off, `weather_forecast` unless `WEATHER` is off,
    /// `show_map` unless `MAPS` is off, and `browser` if `BROWSER` is on.
    pub fn from_config(cfg: &Config) -> Result<Self> {
        let mut set = Self::standard();
        if let Some(key) = &cfg.brave_api_key {
            set.tools.push(Box::new(BraveSearch::new(
                key.clone(),
                cfg.brave_base_url.clone(),
            )?));
        }
        if cfg.web_fetch {
            let client = Arc::new(WebClient::new(cfg.web_fetch_allow_private)?);
            set.tools.push(Box::new(FetchUrl(client.clone())));
            set.tools.push(Box::new(DownloadFile(client)));
        }
        if cfg.weather {
            set.tools.push(Box::new(WeatherForecast::new(
                cfg.weather_base_url.clone(),
                cfg.weather_geocoding_base_url.clone(),
            )?));
        }
        if cfg.maps {
            set.tools.push(Box::new(ShowMap::new(
                cfg.weather_geocoding_base_url.clone(),
            )?));
        }
        if cfg.browser.enabled {
            set.tools
                .push(Box::new(BrowserTool::new(cfg.web_fetch_allow_private)));
        }
        Ok(set)
    }

    /// A note for the system prompt saying what the sandbox can reach and how to use the web, so
    /// the model doesn't try to fetch search engines or pages with `curl`.
    pub fn environment_note(&self, sandbox_network: &str) -> String {
        let search = self.get("web_search").is_some();
        let fetch = self.get("fetch_url").is_some();
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
        let mut note = format!("{net} {how}");
        if fetch {
            note.push_str(
                " To read a web page, use fetch_url; to save a file from the web into the \
                 workspace, use download_file.",
            );
        }
        if self.get("browser").is_some() {
            note.push_str(
                " For pages that only work in a real browser (content loaded by JavaScript, search \
                 forms, buttons that open dialogs or load more), use the browser tool.",
            );
        }
        if self.get("weather_forecast").is_some() {
            note.push_str(
                " For the weather, use weather_forecast; do not search for it or scrape weather sites.",
            );
        }
        if self.get("show_map").is_some() {
            note.push_str(" To show the user where places are, use show_map.");
        }
        note
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

/// `s` as a single-quoted POSIX shell word.
pub(crate) fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// A command's output as the model sees it: stdout, then stderr under a `[stderr]` marker, each
/// cut to `ctx.max_output`, then a note if the command timed out or exited non-zero.
pub(crate) fn exec_report(out: &ExecOutput, ctx: &ToolContext) -> String {
    let stream = |bytes: &[u8], total: usize| {
        truncate_with_total(&String::from_utf8_lossy(bytes), total, ctx.max_output)
    };
    let mut s = stream(&out.stdout, out.stdout_total);
    let err = stream(&out.stderr, out.stderr_total);
    if !err.is_empty() {
        s.push_str("\n[stderr]\n");
        s.push_str(&err);
    }
    match out.exit_code {
        Some(0) => {}
        Some(124) => s.push_str(&format!("\n[timed out after {}s]", ctx.timeout.as_secs())),
        Some(code) => s.push_str(&format!("\n[exit status: {code}]")),
        None => s.push_str("\n[terminated by signal]"),
    }
    s
}

/// The renderer's stderr, cut to `max_output` and trimmed, for an error message or warnings.
pub(crate) fn stderr_text(out: &ExecOutput, max_output: usize) -> String {
    truncate_with_total(
        &String::from_utf8_lossy(&out.stderr),
        out.stderr_total,
        max_output,
    )
    .trim()
    .to_string()
}

/// Append `text` from character `start` to `out`, cut so `out` stays within `max_bytes`. If the
/// text doesn't fit, a note follows saying how much is left and how to read on: `how(next)` names
/// the call, e.g. "call fetch_url with start=1234".
pub(crate) fn append_window(
    out: &mut String,
    text: &str,
    start: usize,
    max_bytes: usize,
    how: impl Fn(usize) -> String,
) {
    let total = text.chars().count();
    if start > 0 && start >= total {
        out.push_str(&format!(
            "[start={start} is past the end of the text ({total} characters)]"
        ));
        return;
    }
    let rest = match text.char_indices().nth(start) {
        Some((i, _)) => &text[i..],
        None => "",
    };
    let budget = max_bytes.saturating_sub(out.len()).max(1);
    let mut end = rest.len().min(budget);
    while !rest.is_char_boundary(end) {
        end -= 1;
    }
    out.push_str(&rest[..end]);
    if end < rest.len() {
        let next = start + rest[..end].chars().count();
        out.push_str(&format!(
            "\n[truncated: {} more characters; {} to continue]",
            total - next,
            how(next)
        ));
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

    const STANDARD: [&str; 7] = [
        "bash",
        "read_file",
        "write_file",
        "run_python",
        "read_document",
        "generate_pdf",
        "generate_chart",
    ];

    #[test]
    fn shell_words_are_single_quoted() {
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote("it's $(x) `y`"), r"'it'\''s $(x) `y`'");
    }

    #[test]
    fn standard_set_exposes_the_local_tools() {
        let set = ToolSet::standard();
        let defs = set.definitions();
        let names: Vec<_> = defs.iter().map(|d| d["name"].as_str().unwrap()).collect();
        assert_eq!(names, STANDARD);
        assert!(set.get("bash").is_some());
        assert!(set.get("nope").is_none());
    }

    fn cfg(pairs: &[(&str, &str)]) -> Config {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Config::from_lookup(move |k| {
            pairs
                .iter()
                .find(|(name, _)| name == k)
                .map(|(_, v)| v.clone())
        })
        .unwrap()
    }

    #[test]
    fn environment_note_points_to_the_web_tools_when_offered() {
        let plain = ToolSet::from_config(&cfg(&[
            ("CEDAR_API_KEYS", "a"),
            ("WEB_FETCH", "off"),
            ("WEATHER", "off"),
            ("MAPS", "off"),
        ]))
        .unwrap();
        let search =
            ToolSet::from_config(&cfg(&[("CEDAR_API_KEYS", "a"), ("BRAVE_API_KEY", "k")])).unwrap();

        let note = search.environment_note("none");
        assert!(note.contains("no internet access"));
        assert!(note.contains("use the web_search tool"));
        assert!(note.contains("use fetch_url") && note.contains("use download_file"));
        assert!(note.contains("use weather_forecast"));
        assert!(note.contains("use show_map"));
        assert!(!note.contains("browser tool"));
        let browser = ToolSet::from_config(&cfg(&[("CEDAR_API_KEYS", "a"), ("BROWSER", "on")]))
            .unwrap()
            .environment_note("none");
        assert!(browser.contains("use the browser tool"), "{browser}");
        let note = plain.environment_note("none");
        assert!(note.contains("No web search tool is available"));
        assert!(!note.contains("use the web_search tool"));
        assert!(!note.contains("fetch_url"));
        assert!(!note.contains("weather_forecast"));
        assert!(!note.contains("show_map"));
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
    fn web_tools_follow_the_configuration() {
        let with = |extra: &[&str]| {
            let mut names: Vec<String> = STANDARD.iter().map(|s| s.to_string()).collect();
            names.extend(extra.iter().map(|s| s.to_string()));
            names
        };
        assert_eq!(
            tool_names(&cfg(&[("CEDAR_API_KEYS", "a")])),
            with(&["fetch_url", "download_file", "weather_forecast", "show_map"])
        );
        assert_eq!(
            tool_names(&cfg(&[("CEDAR_API_KEYS", "a"), ("BRAVE_API_KEY", "k")])),
            with(&[
                "web_search",
                "fetch_url",
                "download_file",
                "weather_forecast",
                "show_map"
            ])
        );
        assert_eq!(
            tool_names(&cfg(&[("CEDAR_API_KEYS", "a"), ("WEB_FETCH", "off")])),
            with(&["weather_forecast", "show_map"])
        );
        assert_eq!(
            tool_names(&cfg(&[("CEDAR_API_KEYS", "a"), ("WEATHER", "off")])),
            with(&["fetch_url", "download_file", "show_map"])
        );
        assert_eq!(
            tool_names(&cfg(&[("CEDAR_API_KEYS", "a"), ("MAPS", "off")])),
            with(&["fetch_url", "download_file", "weather_forecast"])
        );
        assert_eq!(
            tool_names(&cfg(&[
                ("CEDAR_API_KEYS", "a"),
                ("WEB_FETCH", "off"),
                ("WEATHER", "off"),
                ("MAPS", "off")
            ])),
            with(&[])
        );
        assert_eq!(
            tool_names(&cfg(&[("CEDAR_API_KEYS", "a"), ("BROWSER", "on")])),
            with(&[
                "fetch_url",
                "download_file",
                "weather_forecast",
                "show_map",
                "browser"
            ])
        );
    }
}
