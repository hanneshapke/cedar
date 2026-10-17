//! Numbered sources, so the model can cite what it read as `[n]` and the chat can link each
//! number. The numbers belong to a run and go in the order the run read things: each page it
//! fetched, file it downloaded, and search result it saw gets the next number the first time its
//! URL turns up. The model sees the numbers in the tool results; the `tool.result` event lists
//! them under `sources` (its `output` is left as the tool wrote it).

use serde_json::{json, Value};

/// One run's numbered URLs.
#[derive(Debug, Default)]
pub struct Sources {
    urls: Vec<String>,
}

/// What a tool result becomes once numbered: the text for the model, and the sources for the event.
pub struct Numbered {
    pub output: String,
    pub sources: Vec<Value>,
}

impl Sources {
    /// The number of `url`, giving it the next one if it is new.
    fn number(&mut self, url: &str) -> usize {
        match self.urls.iter().position(|u| u == url) {
            Some(i) => i + 1,
            None => {
                self.urls.push(url.to_string());
                self.urls.len()
            }
        }
    }

    /// Number the sources in a successful result of `tool`. `None` for tools that read nothing
    /// from the web.
    pub fn number_result(&mut self, tool: &str, input: &Value, output: &str) -> Option<Numbered> {
        match tool {
            "fetch_url" | "download_file" => {
                // A redirect lands on the page it names in its "URL:" line; that is the source.
                let url = output
                    .lines()
                    .next()
                    .and_then(|l| l.strip_prefix("URL: "))
                    .filter(|_| tool == "fetch_url")
                    .or_else(|| input["url"].as_str())
                    .map(str::trim)
                    .filter(|u| !u.is_empty())?;
                let n = self.number(url);
                let title = if tool == "fetch_url" {
                    output
                        .lines()
                        .find_map(|l| l.strip_prefix("Title: "))
                        .map(str::trim)
                        .filter(|t| !t.is_empty())
                        .unwrap_or(url)
                } else {
                    input["path"]
                        .as_str()
                        .and_then(|p| p.rsplit('/').next())
                        .unwrap_or(url)
                };
                Some(Numbered {
                    output: format!("[{n}] {url}\n{output}"),
                    sources: vec![json!({ "n": n, "url": url, "title": title })],
                })
            }
            "web_search" => {
                let mut sources = Vec::new();
                let output = self.number_search_results(output, &mut sources);
                (!sources.is_empty()).then_some(Numbered { output, sources })
            }
            _ => None,
        }
    }

    /// `web_search` lists results as "1. Title\n   https://url (age)\n   snippet". Each result's
    /// "1." becomes its source number, "[4]".
    fn number_search_results(&mut self, output: &str, sources: &mut Vec<Value>) -> String {
        let lines: Vec<&str> = output.lines().collect();
        let mut out = Vec::with_capacity(lines.len());
        for (i, line) in lines.iter().enumerate() {
            let entry = line
                .split_once(". ")
                .filter(|(num, _)| !num.is_empty() && num.chars().all(|c| c.is_ascii_digit()));
            let url = lines
                .get(i + 1)
                .and_then(|next| next.strip_prefix("   "))
                .and_then(|next| next.split_whitespace().next())
                .filter(|u| u.starts_with("http://") || u.starts_with("https://"));
            match (entry, url) {
                (Some((_, title)), Some(url)) => {
                    let n = self.number(url);
                    sources.push(json!({ "n": n, "url": url, "title": title }));
                    out.push(format!("[{n}] {title}"));
                }
                _ => out.push(line.to_string()),
            }
        }
        out.join("\n")
    }
}

/// The line the environment note gets when the run has a tool that reads the web.
pub const CITE_NOTE: &str = "Cite sources as [n], using the numbers shown with fetched pages, \
    downloaded files and search results.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_follow_reading_order_and_repeat_for_the_same_url() {
        let mut s = Sources::default();
        let search = "1. Alpha\n   https://a.example/x (2 days ago)\n   snippet\n\n2. Beta\n   https://b.example/";
        let r = s.number_result("web_search", &json!({}), search).unwrap();
        assert_eq!(
            r.output,
            "[1] Alpha\n   https://a.example/x (2 days ago)\n   snippet\n\n[2] Beta\n   https://b.example/"
        );
        assert_eq!(
            r.sources[1],
            json!({ "n": 2, "url": "https://b.example/", "title": "Beta" })
        );

        let page = "URL: https://b.example/\nTitle: Beta home\n\ntext";
        let r = s
            .number_result("fetch_url", &json!({ "url": "https://b.example/" }), page)
            .unwrap();
        assert!(r.output.starts_with("[2] https://b.example/\nURL:"));
        assert_eq!(
            r.sources,
            [json!({ "n": 2, "url": "https://b.example/", "title": "Beta home" })]
        );
        // Redirected to a page it already has: the same number.
        let r = s
            .number_result(
                "fetch_url",
                &json!({ "url": "https://b.example/old" }),
                page,
            )
            .unwrap();
        assert_eq!(r.sources[0]["n"], 2);

        let r = s
            .number_result(
                "download_file",
                &json!({ "url": "https://c.example/r.pdf", "path": "attachments/r.pdf" }),
                "Saved",
            )
            .unwrap();
        assert_eq!(
            r.sources,
            [json!({ "n": 3, "url": "https://c.example/r.pdf", "title": "r.pdf" })]
        );
    }

    #[test]
    fn other_tools_and_empty_searches_are_left_alone() {
        let mut s = Sources::default();
        assert!(s
            .number_result("bash", &json!({}), "1. x\n   https://a.b")
            .is_none());
        assert!(s
            .number_result("web_search", &json!({}), "No results.")
            .is_none());
    }
}
