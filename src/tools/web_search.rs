use super::{truncate_output, Tool, ToolContext};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use reqwest::StatusCode;
use serde_json::{json, Value};
use std::time::Duration;

const DEFAULT_COUNT: u64 = 5;
const MAX_COUNT: u64 = 20;
const FRESHNESS: [&str; 4] = ["pd", "pw", "pm", "py"];
/// Brave's free plan allows one request per second, so one retry after this pause is enough.
const RATE_LIMIT_PAUSE: Duration = Duration::from_secs(1);

/// Web search through the Brave Search API.
///
/// Unlike the other tools this one runs in the cedar process, not in the sandbox: the container has
/// no network, and the API key must stay out of it.
pub struct BraveSearch {
    http: reqwest::Client,
    api_key: String,
    base_url: String,
}

impl BraveSearch {
    /// `base_url` is the full web search endpoint (see `DEFAULT_BRAVE_BASE_URL`).
    pub fn new(api_key: String, base_url: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .context("building HTTP client")?;
        Ok(Self {
            http,
            api_key,
            base_url,
        })
    }

    async fn send(&self, search: &Search<'_>, timeout: Duration) -> Result<reqwest::Response> {
        let mut params = vec![
            ("q", search.query.to_string()),
            ("count", search.count.to_string()),
        ];
        if let Some(freshness) = search.freshness {
            params.push(("freshness", freshness.to_string()));
        }
        self.http
            .get(&self.base_url)
            .header("Accept", "application/json")
            .header("X-Subscription-Token", &self.api_key)
            .query(&params)
            .timeout(timeout)
            .send()
            .await
            .context("sending Brave Search request")
    }
}

#[async_trait]
impl Tool for BraveSearch {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web with Brave Search and return the top results as titles, URLs, and snippets. \
         Use it for current information or facts you are unsure about. It does not fetch full pages."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "The search query." },
                    "count": { "type": "integer", "minimum": 1, "maximum": MAX_COUNT,
                               "description": "How many results to return (default 5)." },
                    "freshness": { "type": "string", "enum": FRESHNESS,
                                   "description": "Only results from the past day (pd), week (pw), month (pm), or year (py)." }
                },
                "required": ["query"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let search = parse_input(input)?;

        let mut resp = self.send(&search, ctx.timeout).await?;
        if resp.status() == StatusCode::TOO_MANY_REQUESTS {
            tokio::time::sleep(RATE_LIMIT_PAUSE).await;
            resp = self.send(&search, ctx.timeout).await?;
        }
        let status = resp.status();
        let text = resp.text().await.context("reading Brave Search response")?;
        match status {
            s if s.is_success() => {}
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                bail!("Brave Search rejected the API key ({status})")
            }
            StatusCode::TOO_MANY_REQUESTS => {
                bail!("Brave Search is rate limited; wait a moment before searching again")
            }
            _ => bail!("Brave Search error {status}: {}", error_message(&text)),
        }

        let body: Value = serde_json::from_str(&text).context("parsing Brave Search response")?;
        Ok(truncate_output(
            &format_results(&body, search.count as usize),
            ctx.max_output,
        ))
    }
}

#[derive(Debug)]
struct Search<'a> {
    query: &'a str,
    count: u64,
    freshness: Option<&'a str>,
}

fn parse_input(input: &Value) -> Result<Search<'_>> {
    let query = input["query"]
        .as_str()
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .ok_or_else(|| anyhow!("missing query"))?;
    let count = input["count"]
        .as_u64()
        .unwrap_or(DEFAULT_COUNT)
        .clamp(1, MAX_COUNT);
    let freshness = match input["freshness"].as_str() {
        None => None,
        Some(f) if FRESHNESS.contains(&f) => Some(f),
        Some(f) => bail!("invalid freshness {f:?}; use one of pd, pw, pm, py"),
    };
    Ok(Search {
        query,
        count,
        freshness,
    })
}

/// A numbered list of `web.results`, or a note that there were none.
fn format_results(body: &Value, count: usize) -> String {
    let results = body["web"]["results"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    if results.is_empty() {
        return "No results.".to_string();
    }
    results
        .iter()
        .take(count)
        .enumerate()
        .map(|(i, r)| {
            let title = clean(r["title"].as_str().unwrap_or("(untitled)"));
            let url = r["url"].as_str().unwrap_or_default();
            let mut entry = format!("{}. {title}\n   {url}", i + 1);
            if let Some(age) = r["age"].as_str() {
                entry.push_str(&format!(" ({age})"));
            }
            let snippet = clean(r["description"].as_str().unwrap_or_default());
            if !snippet.is_empty() {
                entry.push_str("\n   ");
                entry.push_str(&snippet);
            }
            entry
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Brave marks matched terms with `<strong>` and HTML-escapes the rest; turn that into plain text.
fn clean(s: &str) -> String {
    let mut text = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        // Only treat `<` as a tag when a tag name follows, so "a < b" survives.
        if c == '<'
            && chars
                .peek()
                .is_some_and(|n| n.is_ascii_alphabetic() || *n == '/')
        {
            for t in chars.by_ref() {
                if t == '>' {
                    break;
                }
            }
        } else {
            text.push(c);
        }
    }
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The detail from Brave's `{"error": {"detail": ...}}` body, or a prefix of the raw body.
fn error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            v["error"]["detail"]
                .as_str()
                .or_else(|| v["error"]["message"].as_str())
                .map(String::from)
        })
        .unwrap_or_else(|| truncate_output(body.trim(), 300))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_are_numbered_with_url_age_and_snippet() {
        let body = json!({ "web": { "results": [
            { "title": "Rust", "url": "https://rust-lang.org", "description": "A language", "age": "2 days ago" },
            { "title": "Crates", "url": "https://crates.io" },
        ] } });
        assert_eq!(
            format_results(&body, 5),
            "1. Rust\n   https://rust-lang.org (2 days ago)\n   A language\n\n2. Crates\n   https://crates.io"
        );
    }

    #[test]
    fn html_markup_and_entities_become_plain_text() {
        let body = json!({ "web": { "results": [
            { "title": "The <strong>Rust</strong> book", "url": "https://x.test",
              "description": "Tom &amp; Jerry&#x27;s <strong>guide</strong>\n to a &lt; b" },
        ] } });
        assert_eq!(
            format_results(&body, 5),
            "1. The Rust book\n   https://x.test\n   Tom & Jerry's guide to a < b"
        );
        assert_eq!(clean("1 < 2 and 3 > 2"), "1 < 2 and 3 > 2");
    }

    #[test]
    fn no_results_says_so() {
        assert_eq!(format_results(&json!({}), 5), "No results.");
        assert_eq!(
            format_results(&json!({ "web": { "results": [] } }), 5),
            "No results."
        );
    }

    #[test]
    fn extra_results_beyond_count_are_dropped() {
        let body = json!({ "web": { "results": [
            { "title": "a", "url": "u1" }, { "title": "b", "url": "u2" },
        ] } });
        assert!(!format_results(&body, 1).contains("u2"));
    }

    #[test]
    fn input_defaults_and_clamps() {
        let input = json!({ "query": "  rust  " });
        let s = parse_input(&input).unwrap();
        assert_eq!((s.query, s.count, s.freshness), ("rust", 5, None));

        let input = json!({ "query": "q", "count": 500, "freshness": "pw" });
        let s = parse_input(&input).unwrap();
        assert_eq!((s.count, s.freshness), (20, Some("pw")));
        assert_eq!(
            parse_input(&json!({ "query": "q", "count": 0 }))
                .unwrap()
                .count,
            1
        );
    }

    #[test]
    fn bad_input_is_rejected() {
        assert!(parse_input(&json!({})).is_err());
        assert!(parse_input(&json!({ "query": "   " })).is_err());
        assert!(parse_input(&json!({ "query": 7 })).is_err());
        let err = parse_input(&json!({ "query": "q", "freshness": "decade" })).unwrap_err();
        assert!(err.to_string().contains("freshness"));
    }

    #[test]
    fn error_detail_is_extracted() {
        let body = r#"{"type":"ErrorResponse","error":{"code":"VALIDATION","detail":"query too long","status":422}}"#;
        assert_eq!(error_message(body), "query too long");
        assert_eq!(error_message("  upstream exploded "), "upstream exploded");
    }
}
