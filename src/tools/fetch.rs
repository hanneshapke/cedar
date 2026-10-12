//! `fetch_url` and `download_file`: the model's way to the web.
//!
//! Like `web_search`, these run in the cedar process rather than the sandbox, which usually has no
//! network. That puts the requests on the host's network, so by default they may only reach public
//! addresses: hostnames are resolved by [`PublicOnly`], which drops loopback, private, link-local,
//! and other internal addresses, and IP literals (in the URL or in a redirect) are checked the same
//! way. Proxy settings from the environment are ignored, since a proxy would resolve the name
//! itself and bypass that check.

use super::{append_window, Tool, ToolContext};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::{CONTENT_LENGTH, CONTENT_TYPE};
use reqwest::{redirect, Url};
use serde_json::{json, Value};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

/// Most bytes of a page `fetch_url` reads; the text is cut to the tool output limit afterwards.
const MAX_PAGE_BYTES: usize = 5 * 1024 * 1024;
/// Largest file `download_file` saves.
pub const MAX_DOWNLOAD_BYTES: usize = 25 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
/// Width the HTML is laid out at; wide enough that prose isn't hard-wrapped mid-sentence often.
const TEXT_WIDTH: usize = 120;
const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (compatible; cedar/",
    env!("CARGO_PKG_VERSION"),
    "; +https://github.com/hanneshapke/cedar)"
);

/// An HTTP client for the model's requests, shared by `fetch_url` and `download_file`.
pub struct WebClient {
    http: reqwest::Client,
    allow_private: bool,
}

impl WebClient {
    /// `allow_private` lifts the public-addresses-only rule (for tests and intranet deployments).
    pub fn new(allow_private: bool) -> Result<Self> {
        let policy = redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.error(format!("more than {MAX_REDIRECTS} redirects"))
            } else if let Err(e) = check_url(attempt.url(), allow_private) {
                let msg = format!("redirected to {}: {e}", attempt.url());
                attempt.error(msg)
            } else {
                attempt.follow()
            }
        });
        let mut builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .user_agent(USER_AGENT)
            .redirect(policy)
            .no_proxy();
        if !allow_private {
            builder = builder.dns_resolver(Arc::new(PublicOnly));
        }
        let http = builder.build().context("building HTTP client")?;
        Ok(Self {
            http,
            allow_private,
        })
    }

    /// GET `url`, reading at most `max_bytes` of the body. Non-2xx statuses are errors. With
    /// `partial` false, a body the server declares too large isn't read at all.
    async fn get(
        &self,
        url: &str,
        max_bytes: usize,
        partial: bool,
        timeout: Duration,
    ) -> Result<Fetched> {
        let url = parse_url(url, self.allow_private)?;
        let mut resp = self
            .http
            .get(url.clone())
            .timeout(timeout)
            .send()
            .await
            .map_err(|e| request_error(&url, e))?;
        let status = resp.status();
        if !status.is_success() {
            bail!("{url} returned HTTP {status}");
        }
        let final_url = resp.url().to_string();
        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        let declared: Option<usize> = resp
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok()?.parse().ok());

        let mut body = Vec::new();
        let mut truncated = !partial && declared.is_some_and(|n| n > max_bytes);
        if !truncated {
            while let Some(chunk) = resp.chunk().await.map_err(|e| request_error(&url, e))? {
                let room = max_bytes - body.len();
                if chunk.len() > room {
                    body.extend_from_slice(&chunk[..room]);
                    truncated = true;
                    break;
                }
                body.extend_from_slice(&chunk);
            }
        }
        Ok(Fetched {
            final_url,
            content_type,
            body,
            truncated,
        })
    }
}

struct Fetched {
    /// After redirects.
    final_url: String,
    /// Lowercased `Content-Type` header, parameters included; empty if absent.
    content_type: String,
    body: Vec<u8>,
    /// The body was longer than the limit (and, without `partial`, `body` is empty if the server
    /// said so up front).
    truncated: bool,
}

/// reqwest's errors nest the useful part ("dns error: ...", "operation timed out") a few levels
/// down; flatten the chain into one line.
fn request_error(url: &Url, e: reqwest::Error) -> anyhow::Error {
    if e.is_timeout() {
        return anyhow!("fetching {url} timed out");
    }
    let mut msg = String::new();
    let mut source: Option<&dyn std::error::Error> = Some(&e);
    while let Some(err) = source {
        let part = err.to_string();
        if !msg.contains(&part) {
            if !msg.is_empty() {
                msg.push_str(": ");
            }
            msg.push_str(&part);
        }
        source = err.source();
    }
    anyhow!("fetching {url} failed: {msg}")
}

fn parse_url(raw: &str, allow_private: bool) -> Result<Url> {
    let url = Url::parse(raw.trim()).map_err(|e| anyhow!("invalid url {raw:?}: {e}"))?;
    check_url(&url, allow_private)?;
    Ok(url)
}

/// [`parse_url`], plus resolving the hostname now and refusing it unless every address is public.
/// For requests cedar doesn't make itself (the browser's), where [`PublicOnly`] can't be applied
/// at connection time. A name that changes its answer later is not caught by this.
pub(crate) async fn check_public_url(raw: &str, allow_private: bool) -> Result<Url> {
    let url = parse_url(raw, allow_private)?;
    if allow_private {
        return Ok(url);
    }
    let host = url.host_str().unwrap_or_default();
    // IP literals were checked by `parse_url`; only names are left to resolve.
    if host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .is_err()
    {
        let port = url.port_or_known_default().unwrap_or(443);
        let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, port))
            .await
            .map_err(|e| anyhow!("could not resolve {host}: {e}"))?
            .collect();
        if addrs.is_empty() || addrs.iter().any(|a| !is_public(a.ip())) {
            bail!("{host} does not resolve to a public address");
        }
    }
    Ok(url)
}

/// Only http(s), and no IP literal outside the public internet unless `allow_private`.
/// Hostnames are checked when they are resolved (see [`PublicOnly`]).
fn check_url(url: &Url, allow_private: bool) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https") {
        bail!("only http and https URLs can be fetched");
    }
    let host = url.host_str().ok_or_else(|| anyhow!("url has no host"))?;
    if !allow_private {
        let literal = host.trim_start_matches('[').trim_end_matches(']');
        if let Ok(ip) = literal.parse::<IpAddr>() {
            if !is_public(ip) {
                bail!("{ip} is not a public address");
            }
        }
    }
    Ok(())
}

/// Resolves names with the system resolver and keeps only public addresses, so a hostname that
/// points at the host itself or the internal network can't be fetched.
struct PublicOnly;

impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|a| is_public(a.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(format!("{host} does not resolve to a public address").into());
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// Is `ip` a globally routable unicast address? (`IpAddr::is_global` is still unstable.)
fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            let seg = v6.segments();
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                || (seg[0] & 0xfe00) == 0xfc00 // unique local
                || (seg[0] & 0xffc0) == 0xfe80 // link local
                || (seg[0] & 0xffc0) == 0xfec0 // site local (deprecated)
                || (seg[0] == 0x2001 && seg[1] == 0x0db8) // documentation
                || (seg[0] == 0x0064 && seg[1] == 0xff9b) // NAT64, may reach IPv4 internals
                || (seg[0] == 0x2002) // 6to4, embeds an IPv4 address
                || seg[..6] == [0; 6]) // IPv4-compatible (deprecated)
        }
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || a == 0
        || (a == 100 && (64..128).contains(&b)) // shared address space (CGNAT)
        || (a == 192 && b == 0 && c == 0) // IETF protocol assignments
        || (a == 198 && (18..20).contains(&b)) // benchmarking
        || a >= 240) // reserved
}

// ---------------------------------------------------------------------------------------------
// fetch_url

pub struct FetchUrl(pub Arc<WebClient>);

#[async_trait]
impl Tool for FetchUrl {
    fn name(&self) -> &str {
        "fetch_url"
    }

    fn description(&self) -> &str {
        "Fetch a web page and return its text: HTML is converted to readable plain text with links \
         listed as numbered references; plain text, Markdown, JSON, XML, and CSV are returned as-is. \
         Use it to read a page found with web_search or a URL the user gave. Long pages are cut \
         off; call again with `start` to read further. For PDFs, images, and other binary files, \
         use download_file and then read_document."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "The http or https URL to fetch." },
                    "start": { "type": "integer", "minimum": 0,
                               "description": "Character offset in the extracted text to start from (default 0), to continue a page that was cut off." }
                },
                "required": ["url"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let url = input["url"]
            .as_str()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .ok_or_else(|| anyhow!("missing url"))?;
        let start = match &input["start"] {
            Value::Null => 0,
            v => v
                .as_u64()
                .ok_or_else(|| anyhow!("start must be a non-negative integer"))?
                as usize,
        };
        let page = self.0.get(url, MAX_PAGE_BYTES, true, ctx.timeout).await?;
        let kind = TextKind::of(&page.content_type, &page.body).ok_or_else(|| {
            anyhow!(
                "{} is {}, not text; use download_file to save it to the workspace (and \
                 read_document to read a PDF, Word, Excel, or PowerPoint file)",
                page.final_url,
                media_type(&page.content_type)
            )
        })?;
        let raw = String::from_utf8_lossy(&page.body);
        let (title, text) = match kind {
            TextKind::Html => (html_title(&raw), html_to_text(&page.body)?),
            TextKind::Plain => (None, raw.into_owned()),
        };
        Ok(format_page(
            &page,
            title.as_deref(),
            &text,
            start,
            ctx.max_output,
        ))
    }
}

#[derive(Debug, PartialEq)]
enum TextKind {
    Html,
    Plain,
}

impl TextKind {
    /// How to read a body of `content_type`, or `None` if it isn't text. Without a content type,
    /// sniff: markup that starts like HTML is HTML, other valid UTF-8 is text.
    fn of(content_type: &str, body: &[u8]) -> Option<Self> {
        let media = media_type(content_type);
        if media == "text/html" || media == "application/xhtml+xml" {
            return Some(Self::Html);
        }
        if media.starts_with("text/")
            || media.ends_with("+json")
            || media.ends_with("+xml")
            || matches!(
                media,
                "application/json"
                    | "application/xml"
                    | "application/javascript"
                    | "application/x-ndjson"
                    | "application/yaml"
                    | "application/x-yaml"
                    | "application/toml"
            )
        {
            return Some(Self::Plain);
        }
        if media.is_empty() || media == "application/octet-stream" {
            let head = String::from_utf8_lossy(&body[..body.len().min(512)]).to_ascii_lowercase();
            let head = head.trim_start();
            if head.starts_with("<!doctype html") || head.starts_with("<html") {
                return Some(Self::Html);
            }
            if std::str::from_utf8(body).is_ok() && !body.contains(&0) {
                return Some(Self::Plain);
            }
        }
        None
    }
}

/// The media type of a `Content-Type` value, without parameters.
fn media_type(content_type: &str) -> &str {
    content_type.split(';').next().unwrap_or_default().trim()
}

fn html_to_text(html: &[u8]) -> Result<String> {
    let text = html2text::config::plain()
        .no_link_wrapping()
        .allow_width_overflow()
        .string_from_read(html, TEXT_WIDTH)
        .context("converting the page to text")?;
    // Trailing spaces from the layout and runs of blank lines only cost tokens.
    let mut out = String::with_capacity(text.len());
    let mut blank = 0;
    for line in text.lines().map(str::trim_end) {
        blank = if line.is_empty() { blank + 1 } else { 0 };
        if blank < 2 {
            out.push_str(line);
            out.push('\n');
        }
    }
    Ok(out.trim().to_string())
}

/// The text of the first `<title>` element, with whitespace collapsed and common entities decoded.
fn html_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let open = lower.find("<title")?;
    let start = open + lower[open..].find('>')? + 1;
    let end = start + lower[start..].find("</title")?;
    let title = html[start..end]
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    (!title.is_empty()).then_some(title)
}

/// The tool result: a short header, then the text from character `start`, cut to fit `max_bytes`
/// with a note saying where to continue.
fn format_page(
    page: &Fetched,
    title: Option<&str>,
    text: &str,
    start: usize,
    max_bytes: usize,
) -> String {
    let mut out = format!("URL: {}\n", page.final_url);
    if let Some(title) = title {
        out.push_str(&format!("Title: {title}\n"));
    }
    if page.truncated {
        out.push_str(&format!(
            "[the page is larger than {} MB; only its beginning was read]\n",
            MAX_PAGE_BYTES / (1024 * 1024)
        ));
    }
    out.push('\n');
    append_window(&mut out, text, start, max_bytes, |next| {
        format!("call fetch_url with start={next}")
    });
    out
}

// ---------------------------------------------------------------------------------------------
// download_file

pub struct DownloadFile(pub Arc<WebClient>);

#[async_trait]
impl Tool for DownloadFile {
    fn name(&self) -> &str {
        "download_file"
    }

    fn description(&self) -> &str {
        "Download a file from an http or https URL into the workspace (PDFs, spreadsheets, images, \
         datasets, archives; up to 25 MB). Use it when a task needs a file from the web, since \
         the sandbox itself may be offline."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "The http or https URL to download." },
                    "path": { "type": "string",
                              "description": "Where to save the file, relative to /workspace. Parent directories are created." }
                },
                "required": ["url", "path"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let url = input["url"]
            .as_str()
            .map(str::trim)
            .filter(|u| !u.is_empty())
            .ok_or_else(|| anyhow!("missing url"))?;
        let path = input["path"]
            .as_str()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .ok_or_else(|| anyhow!("missing path"))?;
        if path.contains('\0') {
            bail!("path must not contain NUL bytes");
        }
        if path.ends_with('/') {
            bail!("path must name a file, not a directory");
        }
        let file = self
            .0
            .get(url, MAX_DOWNLOAD_BYTES, false, ctx.timeout)
            .await?;
        if file.truncated {
            bail!(
                "{} is larger than the {} MB download limit",
                file.final_url,
                MAX_DOWNLOAD_BYTES / (1024 * 1024)
            );
        }
        ctx.sandbox
            .write_file(path, &file.body)
            .await
            .with_context(|| format!("saving {path}"))?;
        let mut msg = format!(
            "saved {} to {path}: {} bytes",
            file.final_url,
            file.body.len()
        );
        let media = media_type(&file.content_type);
        if !media.is_empty() {
            msg.push_str(&format!(", {media}"));
        }
        Ok(msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(content_type: &str) -> Fetched {
        Fetched {
            final_url: "https://example.com/a".into(),
            content_type: content_type.into(),
            body: vec![],
            truncated: false,
        }
    }

    #[test]
    fn internal_addresses_are_not_public() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "100.64.0.1",
            "255.255.255.255",
            "224.0.0.1",
            "::1",
            "::",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "64:ff9b::a00:1",
            "2002:7f00:1::",
            "::127.0.0.1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "93.184.216.34",
            "1.1.1.1",
            "2606:4700:4700::1111",
            "::ffff:1.1.1.1",
        ] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn urls_must_be_http_and_public_unless_allowed() {
        let err = |u: &str| parse_url(u, false).unwrap_err().to_string();
        assert!(parse_url("https://example.com/x?y=1", false).is_ok());
        assert!(err("file:///etc/passwd").contains("only http and https"));
        assert!(err("ftp://example.com/").contains("only http and https"));
        assert!(err("not a url").contains("invalid url"));
        assert!(err("http://127.0.0.1:8080/").contains("not a public address"));
        assert!(err("http://[::1]/").contains("not a public address"));
        // The URL parser normalizes odd IPv4 spellings, so they are caught too.
        assert!(err("http://0x7f.1/").contains("not a public address"));
        assert!(err("http://2130706433/").contains("not a public address"));
        assert!(parse_url("http://127.0.0.1:8080/", true).is_ok());
    }

    #[tokio::test]
    async fn hostnames_resolving_to_internal_addresses_are_refused() {
        let name: Name = "localhost".parse().unwrap();
        let err = PublicOnly.resolve(name).await.err().unwrap();
        assert!(err.to_string().contains("public address"), "{err}");
    }

    #[test]
    fn content_types_decide_how_a_body_is_read() {
        assert_eq!(
            TextKind::of("text/html; charset=utf-8", b""),
            Some(TextKind::Html)
        );
        assert_eq!(TextKind::of("text/plain", b""), Some(TextKind::Plain));
        assert_eq!(
            TextKind::of("application/ld+json", b""),
            Some(TextKind::Plain)
        );
        assert_eq!(TextKind::of("application/json", b""), Some(TextKind::Plain));
        assert_eq!(TextKind::of("application/pdf", b"%PDF-1.7"), None);
        assert_eq!(TextKind::of("image/png", b""), None);
        assert_eq!(
            TextKind::of("", b"  <!DOCTYPE html><p>x"),
            Some(TextKind::Html)
        );
        assert_eq!(
            TextKind::of("application/octet-stream", b"a,b\n1,2\n"),
            Some(TextKind::Plain)
        );
        assert_eq!(TextKind::of("", b"\x00\x01\x02"), None);
    }

    #[test]
    fn html_becomes_text_without_scripts_or_styles() {
        let html = b"<html><head><title>T</title><style>p{color:red}</style>\
            <script>alert(1)</script></head><body><h1>Hello</h1>\
            <p>Some <b>bold</b> text and a <a href=\"https://x.test/\">link</a>.</p>\
            <ul><li>one</li><li>two</li></ul></body></html>";
        let text = html_to_text(html).unwrap();
        assert!(text.contains("Hello"), "{text}");
        assert!(text.contains("**bold**"), "{text}");
        assert!(text.contains("one") && text.contains("two"), "{text}");
        assert!(text.contains("https://x.test/"), "{text}");
        assert!(
            !text.contains("alert") && !text.contains("color:red"),
            "{text}"
        );
        assert!(!text.contains("\n\n\n"), "{text}");
    }

    #[test]
    fn titles_are_extracted_and_cleaned() {
        assert_eq!(
            html_title("<html><HEAD><Title lang=en>\n  Rust &amp; Co\n</TITLE>").as_deref(),
            Some("Rust & Co")
        );
        assert_eq!(html_title("<title></title>"), None);
        assert_eq!(html_title("<p>no title</p>"), None);
    }

    #[test]
    fn long_text_is_cut_with_a_continuation_offset() {
        let p = page("text/plain");
        let text = "é".repeat(100); // 200 bytes
        let out = format_page(&p, Some("T"), &text, 0, 80);
        let header = "URL: https://example.com/a\nTitle: T\n\n";
        assert!(out.starts_with(header), "{out}");
        let shown = (80 - header.len()) / 2;
        assert!(
            out.ends_with(&format!(
                "[truncated: {} more characters; call fetch_url with start={shown} to continue]",
                100 - shown
            )),
            "{out}"
        );

        let out = format_page(&p, None, &text, 90, 1000);
        assert_eq!(
            out,
            format!("URL: https://example.com/a\n\n{}", "é".repeat(10))
        );
        let out = format_page(&p, None, &text, 100, 1000);
        assert!(
            out.contains("past the end of the text (100 characters)"),
            "{out}"
        );
        assert_eq!(
            format_page(&p, None, "", 0, 1000),
            "URL: https://example.com/a\n\n"
        );
    }

    #[test]
    fn an_oversized_page_says_only_its_beginning_was_read() {
        let p = Fetched {
            truncated: true,
            ..page("text/plain")
        };
        assert!(format_page(&p, None, "abc", 0, 1000).contains("only its beginning was read"));
    }
}
