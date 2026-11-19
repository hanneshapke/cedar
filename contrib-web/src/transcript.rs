//! A conversation as the importers read it, before it becomes a bundle: one list of items in the
//! order they happened, on the branch the person last looked at. Both exports (ChatGPT's and
//! Claude's) are reduced to this, so the bundle is built, redacted and checked one way.

use serde_json::Value;
use std::collections::BTreeSet;

/// Where an export came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    ChatGpt,
    Claude,
}

impl Source {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "chatgpt" => Some(Source::ChatGpt),
            "claude" => Some(Source::Claude),
            _ => None,
        }
    }

    /// The bundle's `model.id`: marks a bundle as imported from an export rather than run by cedar.
    pub fn model_id(self) -> &'static str {
        match self {
            Source::ChatGpt => "chatgpt-export",
            Source::Claude => "claude-export",
        }
    }

    /// The prefix every model name from this source carries, so a bundle can't claim another
    /// provider's model.
    pub fn vendor(self) -> &'static str {
        match self {
            Source::ChatGpt => "openai",
            Source::Claude => "anthropic",
        }
    }

    pub fn tool_description(self) -> &'static str {
        match self {
            Source::ChatGpt => "A tool built into ChatGPT. Its definition is not part of the export.",
            Source::Claude => "A tool built into Claude. Its definition is not part of the export.",
        }
    }
}

/// A file attached to a prompt: only its type and size go, never its name or bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct FileStub {
    /// `image` or `document`.
    pub kind: &'static str,
    pub media_type: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// Something the person typed, which starts an exchange.
    Prompt { text: String, day: Option<String>, files: Vec<FileStub> },
    /// Text the model showed.
    Text { text: String },
    /// The model's reasoning (o-series thoughts, Claude's thinking blocks).
    Reasoning { text: String },
    ToolCall { id: String, name: String, input: Value },
    ToolResult { id: String, text: String, is_error: bool },
}

/// One imported conversation.
#[derive(Debug, Clone)]
pub struct Transcript {
    pub source: Source,
    /// The model's name as the export gives it (`gpt-4o`, `claude-sonnet-4-5`), if it does.
    pub model: Option<String>,
    /// The day the conversation started, when no prompt carries one.
    pub day: Option<String>,
    pub items: Vec<Item>,
    /// Counts of what the importer left out.
    pub dropped: cedar_contrib_schema::Dropped,
    /// Tools that read the person's mail, calendar, drives and other accounts: other people's
    /// data, so the conversation can't be contributed.
    pub private_tools: BTreeSet<String>,
    /// Tool results that were pages or files from elsewhere, by call id, with the tool's name.
    pub fetched: BTreeSet<String>,
    /// Tool results that were the person's own uploaded files, by call id: always left out.
    pub files: BTreeSet<String>,
}

impl Transcript {
    pub fn new(source: Source) -> Self {
        Transcript {
            source,
            model: None,
            day: None,
            items: Vec::new(),
            dropped: Default::default(),
            private_tools: BTreeSet::new(),
            fetched: BTreeSet::new(),
            files: BTreeSet::new(),
        }
    }
}

/// What kind of tool a name is, for what happens to its results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolKind {
    /// Reads the person's accounts (mail, calendar, drives, chat apps): blocks the conversation.
    Private,
    /// Reads or writes the assistant's memory of the person: the call and its result are cut.
    Memory,
    /// Searches the person's own uploaded files: results always left out.
    Files,
    /// Fetches pages from the web: results left out unless the person keeps them.
    Fetching,
    Other,
}

pub fn tool_kind(name: &str) -> ToolKind {
    let n = name.to_ascii_lowercase();
    const PRIVATE: [&str; 14] = [
        "mail", "calendar", "gcal", "outlook", "drive", "sharepoint", "onedrive", "dropbox", "slack", "notion", "github", "contacts", "teams", "connector",
    ];
    const MEMORY: [&str; 5] = ["bio", "memory", "conversation_search", "recent_chats", "personal_context"];
    if PRIVATE.iter().any(|p| n.contains(p)) || n.starts_with("api_tool") || n.starts_with("mcp") {
        ToolKind::Private
    } else if MEMORY.iter().any(|m| n == *m || n.starts_with(&format!("{m}.")) || n.starts_with(&format!("{m}_"))) {
        ToolKind::Memory
    } else if n.contains("myfiles") || n.contains("file_search") || n.contains("files") {
        ToolKind::Files
    } else if n.contains("browser") || n.starts_with("web") || n.contains("fetch") || n.contains("search") {
        ToolKind::Fetching
    } else {
        ToolKind::Other
    }
}

/// `YYYY-MM-DD` of a Unix time in seconds (UTC).
pub fn day_of_epoch(secs: f64) -> Option<String> {
    if !secs.is_finite() || secs < 0.0 {
        return None;
    }
    let days = (secs / 86_400.0).floor() as i64;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

/// `YYYY-MM-DD` from the start of an ISO 8601 timestamp.
pub fn day_of_iso(s: &str) -> Option<String> {
    let d = s.get(..10)?;
    let b = d.as_bytes();
    let ok = b[4] == b'-' && b[7] == b'-' && b.iter().enumerate().all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    ok.then(|| d.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_from_epoch_and_iso() {
        assert_eq!(day_of_epoch(0.0).as_deref(), Some("1970-01-01"));
        assert_eq!(day_of_epoch(1_730_419_200.5).as_deref(), Some("2024-11-01"));
        assert_eq!(day_of_epoch(951_782_400.0).as_deref(), Some("2000-02-29"));
        assert_eq!(day_of_iso("2025-03-04T10:11:12.000Z").as_deref(), Some("2025-03-04"));
        assert_eq!(day_of_iso("nope"), None);
    }

    #[test]
    fn tools_are_sorted_by_what_their_results_are() {
        assert_eq!(tool_kind("gmail_search"), ToolKind::Private);
        assert_eq!(tool_kind("google_calendar_search"), ToolKind::Private);
        assert_eq!(tool_kind("bio"), ToolKind::Memory);
        assert_eq!(tool_kind("myfiles_browser"), ToolKind::Files);
        assert_eq!(tool_kind("file_search"), ToolKind::Files);
        assert_eq!(tool_kind("browser"), ToolKind::Fetching);
        assert_eq!(tool_kind("web.run"), ToolKind::Fetching);
        assert_eq!(tool_kind("web_fetch"), ToolKind::Fetching);
        assert_eq!(tool_kind("python"), ToolKind::Other);
        assert_eq!(tool_kind("dalle.text2im"), ToolKind::Other);
        assert_eq!(tool_kind("artifacts"), ToolKind::Other);
    }
}
