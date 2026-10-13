//! Gmail through the REST API (`GOOGLE_BACKEND=api`), with tools named like those of Google's
//! Gmail MCP server. Like `web_search` they run in the cedar process: the OAuth tokens stay here,
//! and the sandbox has no network.
//!
//! Only reading and drafting are implemented; nothing here sends, deletes, or relabels mail.

use super::fetch::html_to_text;
use super::google::{page_size, required_str, select, str_arg, str_list, without_empty, GoogleApi};
use super::{truncate_output, Tool, ToolContext};
use crate::config::GoogleApiConfig;
use crate::mcp::GoogleOAuth;
use anyhow::{bail, Result};
use async_trait::async_trait;
use base64::alphabet;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, STANDARD, URL_SAFE};
use base64::engine::DecodePaddingMode;
use base64::Engine;
use futures_util::{stream, StreamExt, TryStreamExt};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// The tools implemented here, in the order they are offered.
const TOOLS: &[&str] = &[
    "search_threads",
    "get_thread",
    "get_message",
    "list_labels",
    "list_drafts",
    "get_draft",
    "create_draft",
];
/// How many threads or drafts are fetched at once to describe a list of them.
const CONCURRENCY: usize = 8;
/// The body of each message in a thread of several is cut to this many bytes, so one long message
/// doesn't push the rest of the thread out of the output.
const THREAD_BODY_MAX: usize = 20_000;

/// Gmail's base64url, which may or may not be padded.
const BASE64URL: GeneralPurpose = GeneralPurpose::new(
    &alphabet::URL_SAFE,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// The Gmail tools `cfg` allows, sharing one API client.
pub fn tools(cfg: &GoogleApiConfig, auth: Arc<GoogleOAuth>) -> Result<Vec<Box<dyn Tool>>> {
    let api = Arc::new(GoogleApi::new(&cfg.url, auth, "Gmail")?);
    Ok(select(&cfg.prefix, &cfg.tools, TOOLS)
        .into_iter()
        .map(|op| {
            Box::new(GmailTool {
                name: format!("{}_{op}", cfg.prefix),
                op,
                api: api.clone(),
            }) as Box<dyn Tool>
        })
        .collect())
}

/// One Gmail operation; `op` is one of [`TOOLS`].
pub struct GmailTool {
    name: String,
    op: &'static str,
    api: Arc<GoogleApi>,
}

#[async_trait]
impl Tool for GmailTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        match self.op {
            "search_threads" => {
                "Search the user's Gmail and list matching threads, newest first, with subject, \
                 sender, date, message count, and a snippet. `query` uses Gmail search syntax \
                 (e.g. `from:alice@example.com is:unread newer_than:7d`, `subject:invoice \
                 has:attachment`); convert natural-language requests into it. Read a thread with \
                 get_thread."
            }
            "get_thread" => {
                "Read a Gmail thread: every message's sender, recipients, date, subject, labels, \
                 plain-text body, and attachment names. Get thread IDs from search_threads."
            }
            "get_message" => {
                "Read one Gmail message: sender, recipients, date, subject, labels, plain-text \
                 body, and attachment names."
            }
            "list_labels" => {
                "List the user's Gmail labels (system and user-defined) with their IDs."
            }
            "list_drafts" => {
                "List the user's Gmail drafts with recipients, subject, and a snippet. `query` \
                 uses Gmail search syntax."
            }
            "get_draft" => "Read one Gmail draft in full. Get draft IDs from list_drafts.",
            "create_draft" => {
                "Save a new email as a draft in the user's Gmail. It is NOT sent: the user reviews \
                 and sends it. Put plain text (no Markdown) in `body`; `htmlBody` optionally adds \
                 a formatted version. To draft a reply, pass the original message's ID as \
                 `replyToMessageId`: the draft joins its thread, is addressed to the sender unless \
                 `to` is given, and gets a `Re:` subject unless `subject` is given."
            }
            _ => unreachable!("not a Gmail tool: {}", self.op),
        }
    }

    fn schema(&self) -> Value {
        let page = |default: u64| {
            json!({ "type": "integer", "minimum": 1, "maximum": 50,
                    "description": format!("How many to return (default {default}, at most 50).") })
        };
        let page_token = json!({ "type": "string",
            "description": "The nextPageToken of a previous call, to get the next page." });
        let emails = |what: &str| {
            json!({ "type": "array", "items": { "type": "string" },
                    "description": format!("{what}, as plain email addresses (e.g. \"user@example.com\").") })
        };
        match self.op {
            "search_threads" => json!({ "type": "object", "properties": {
                "query": { "type": "string", "description": "A Gmail search query. Omit to list the latest threads." },
                "pageSize": page(10),
                "pageToken": page_token,
                "includeTrash": { "type": "boolean", "description": "Include threads in Trash and Spam (default false)." },
            } }),
            "get_thread" => json!({ "type": "object", "properties": {
                "threadId": { "type": "string", "description": "The thread's ID." },
            }, "required": ["threadId"] }),
            "get_message" => json!({ "type": "object", "properties": {
                "messageId": { "type": "string", "description": "The message's ID." },
            }, "required": ["messageId"] }),
            "list_labels" => json!({ "type": "object", "properties": {} }),
            "list_drafts" => json!({ "type": "object", "properties": {
                "query": { "type": "string", "description": "A Gmail search query to filter drafts." },
                "pageSize": page(20),
                "pageToken": page_token,
            } }),
            "get_draft" => json!({ "type": "object", "properties": {
                "draftId": { "type": "string", "description": "The draft's ID." },
            }, "required": ["draftId"] }),
            "create_draft" => json!({ "type": "object", "properties": {
                "to": emails("Recipients"),
                "cc": emails("Cc recipients"),
                "bcc": emails("Bcc recipients"),
                "subject": { "type": "string", "description": "The subject line." },
                "body": { "type": "string", "description": "The plain-text body. No Markdown." },
                "htmlBody": { "type": "string", "description": "Optional HTML version of the body." },
                "replyToMessageId": { "type": "string", "description": "The ID of the message this draft replies to." },
            } }),
            _ => unreachable!("not a Gmail tool: {}", self.op),
        }
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let t = ctx.timeout;
        let out = match self.op {
            "search_threads" => self.search_threads(input, t).await?,
            "get_thread" => self.get_thread(required_str(input, "threadId")?, t).await?,
            "get_message" => {
                let id = required_str(input, "messageId")?;
                let msg = self.api.get(&["messages", id], &full(), t).await?;
                render_message(&msg, usize::MAX)
            }
            "list_labels" => {
                let list = self.api.get(&["labels"], &[], t).await?;
                let labels: Vec<Value> = items(&list, "labels")
                    .map(|l| json!({ "id": l["id"], "name": l["name"], "type": l["type"] }))
                    .collect();
                json!({ "labels": labels })
            }
            "list_drafts" => self.list_drafts(input, t).await?,
            "get_draft" => {
                let id = required_str(input, "draftId")?;
                let draft = self.api.get(&["drafts", id], &full(), t).await?;
                json!({ "id": draft["id"], "message": render_message(&draft["message"], usize::MAX) })
            }
            "create_draft" => self.create_draft(input, t).await?,
            _ => unreachable!("not a Gmail tool: {}", self.op),
        };
        Ok(truncate_output(
            &serde_json::to_string(&without_empty(out))?,
            ctx.max_output,
        ))
    }
}

impl GmailTool {
    async fn search_threads(&self, input: &Value, t: Duration) -> Result<Value> {
        let mut query = vec![("maxResults", page_size(input, 10, 50).to_string())];
        if let Some(q) = str_arg(input, "query") {
            query.push(("q", q.to_string()));
        }
        if let Some(token) = str_arg(input, "pageToken") {
            query.push(("pageToken", token.to_string()));
        }
        if input["includeTrash"].as_bool() == Some(true) {
            query.push(("includeSpamTrash", "true".into()));
        }
        let list = self.api.get(&["threads"], &query, t).await?;
        let ids: Vec<String> = items(&list, "threads")
            .filter_map(|th| th["id"].as_str().map(String::from))
            .collect();
        let threads: Vec<Value> = stream::iter(ids)
            .map(|id| async move {
                let th = self
                    .api
                    .get(
                        &["threads", &id],
                        &metadata(&["Subject", "From", "Date"]),
                        t,
                    )
                    .await?;
                Ok::<_, anyhow::Error>(thread_summary(&th))
            })
            .buffered(CONCURRENCY)
            .try_collect()
            .await?;
        Ok(json!({ "threads": threads, "nextPageToken": list["nextPageToken"] }))
    }

    async fn get_thread(&self, id: &str, t: Duration) -> Result<Value> {
        let thread = self.api.get(&["threads", id], &full(), t).await?;
        let messages: Vec<&Value> = items(&thread, "messages").collect();
        let body_max = if messages.len() > 1 {
            THREAD_BODY_MAX
        } else {
            usize::MAX
        };
        let rendered: Vec<Value> = messages
            .iter()
            .map(|m| render_message(m, body_max))
            .collect();
        Ok(json!({ "id": thread["id"], "messages": rendered }))
    }

    async fn list_drafts(&self, input: &Value, t: Duration) -> Result<Value> {
        let mut query = vec![("maxResults", page_size(input, 20, 50).to_string())];
        if let Some(q) = str_arg(input, "query") {
            query.push(("q", q.to_string()));
        }
        if let Some(token) = str_arg(input, "pageToken") {
            query.push(("pageToken", token.to_string()));
        }
        let list = self.api.get(&["drafts"], &query, t).await?;
        let ids: Vec<String> = items(&list, "drafts")
            .filter_map(|d| d["id"].as_str().map(String::from))
            .collect();
        let drafts: Vec<Value> = stream::iter(ids)
            .map(|id| async move {
                let d = self
                    .api
                    .get(
                        &["drafts", &id],
                        &metadata(&["To", "Cc", "Subject", "Date"]),
                        t,
                    )
                    .await?;
                let m = &d["message"];
                let p = &m["payload"];
                Ok::<_, anyhow::Error>(json!({
                    "id": d["id"], "messageId": m["id"], "threadId": m["threadId"],
                    "to": header(p, "To"), "cc": header(p, "Cc"), "subject": header(p, "Subject"),
                    "date": header(p, "Date"), "snippet": m["snippet"],
                }))
            })
            .buffered(CONCURRENCY)
            .try_collect()
            .await?;
        Ok(json!({ "drafts": drafts, "nextPageToken": list["nextPageToken"] }))
    }

    async fn create_draft(&self, input: &Value, t: Duration) -> Result<Value> {
        let mut to = str_list(input, "to");
        let cc = str_list(input, "cc");
        let bcc = str_list(input, "bcc");
        for address in to.iter().chain(&cc).chain(&bcc) {
            check_address(address)?;
        }
        let mut subject = str_arg(input, "subject").map(String::from);
        let mut headers = Vec::new();
        let mut thread_id = None;
        if let Some(reply_to) = str_arg(input, "replyToMessageId") {
            let original = self
                .api
                .get(
                    &["messages", reply_to],
                    &metadata(&["Message-ID", "References", "Subject", "From", "Reply-To"]),
                    t,
                )
                .await?;
            let p = &original["payload"];
            thread_id = original["threadId"].as_str().map(String::from);
            if to.is_empty() {
                if let Some(sender) = header(p, "Reply-To").or_else(|| header(p, "From")) {
                    to.push(address_of(sender).to_string());
                }
            }
            if subject.is_none() {
                let original_subject = header(p, "Subject").unwrap_or_default();
                subject = Some(
                    if original_subject.to_ascii_lowercase().starts_with("re:") {
                        original_subject.to_string()
                    } else {
                        format!("Re: {original_subject}")
                    },
                );
            }
            if let Some(message_id) = header(p, "Message-ID") {
                let references = match header(p, "References") {
                    Some(r) => format!("{r} {message_id}"),
                    None => message_id.to_string(),
                };
                headers.push(("In-Reply-To", message_id.to_string()));
                headers.push(("References", references));
            }
        }
        for (name, list) in [("To", &to), ("Cc", &cc), ("Bcc", &bcc)] {
            if !list.is_empty() {
                headers.push((name, list.join(", ")));
            }
        }
        headers.push(("Subject", subject.unwrap_or_default()));
        let raw = mime_message(&headers, input["body"].as_str(), str_arg(input, "htmlBody"))?;
        let mut message = json!({ "raw": URL_SAFE.encode(raw) });
        if let Some(id) = thread_id {
            message["threadId"] = json!(id);
        }
        let draft = self
            .api
            .post(&["drafts"], &json!({ "message": message }), t)
            .await?;
        Ok(json!({
            "draftId": draft["id"],
            "messageId": draft["message"]["id"],
            "threadId": draft["message"]["threadId"],
            "note": "Saved as a draft in Gmail. It has not been sent.",
        }))
    }
}

fn full() -> Vec<(&'static str, String)> {
    vec![("format", "full".into())]
}

/// `format=metadata` with only the named headers.
fn metadata(headers: &[&str]) -> Vec<(&'static str, String)> {
    let mut query = vec![("format", "metadata".to_string())];
    query.extend(headers.iter().map(|h| ("metadataHeaders", h.to_string())));
    query
}

fn items<'a>(v: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    v[key].as_array().into_iter().flatten()
}

/// A header's value from a message payload, by case-insensitive name.
fn header<'a>(payload: &'a Value, name: &str) -> Option<&'a str> {
    items(payload, "headers")
        .find(|h| {
            h["name"]
                .as_str()
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
        .and_then(|h| h["value"].as_str())
}

/// A thread described by its first message's subject and sender and its last message's date.
fn thread_summary(thread: &Value) -> Value {
    let messages: Vec<&Value> = items(thread, "messages").collect();
    let (Some(first), Some(last)) = (messages.first(), messages.last()) else {
        return json!({ "id": thread["id"] });
    };
    let unread = messages
        .iter()
        .any(|m| items(m, "labelIds").any(|l| l == "UNREAD"));
    json!({
        "id": thread["id"],
        "subject": header(&first["payload"], "Subject"),
        "from": header(&first["payload"], "From"),
        "lastFrom": (messages.len() > 1).then(|| header(&last["payload"], "From")).flatten(),
        "date": header(&last["payload"], "Date"),
        "messageCount": messages.len(),
        "unread": unread,
        "labels": last["labelIds"],
        "snippet": last["snippet"],
    })
}

/// A message (`format=full`) as the model sees it, its body cut to `body_max` bytes.
fn render_message(msg: &Value, body_max: usize) -> Value {
    let p = &msg["payload"];
    let mut attachments = Vec::new();
    collect_attachments(p, &mut attachments);
    json!({
        "id": msg["id"],
        "threadId": msg["threadId"],
        "date": header(p, "Date"),
        "from": header(p, "From"),
        "to": header(p, "To"),
        "cc": header(p, "Cc"),
        "subject": header(p, "Subject"),
        "labels": msg["labelIds"],
        "body": truncate_output(&body_text(p), body_max),
        "attachments": attachments,
    })
}

/// The message's text: its first `text/plain` part, or else its first `text/html` part as text.
/// A blank plain part, which some senders include beside the real HTML one, counts as none.
fn body_text(payload: &Value) -> String {
    if let Some(part) = find_part(payload, "text/plain") {
        let text = decode(&part["body"]["data"]).trim().to_string();
        if !text.is_empty() {
            return text;
        }
    }
    if let Some(part) = find_part(payload, "text/html") {
        let html = decode(&part["body"]["data"]);
        return html_to_text(html.as_bytes())
            .unwrap_or(html)
            .trim()
            .to_string();
    }
    String::new()
}

fn find_part<'a>(part: &'a Value, mime_type: &str) -> Option<&'a Value> {
    if part["mimeType"] == mime_type && !is_attachment(part) && part["body"]["data"].is_string() {
        return Some(part);
    }
    items(part, "parts").find_map(|p| find_part(p, mime_type))
}

fn is_attachment(part: &Value) -> bool {
    part["filename"].as_str().is_some_and(|f| !f.is_empty())
}

fn collect_attachments(part: &Value, out: &mut Vec<Value>) {
    if is_attachment(part) {
        out.push(json!({
            "filename": part["filename"],
            "mimeType": part["mimeType"],
            "size": part["body"]["size"],
        }));
    }
    for p in items(part, "parts") {
        collect_attachments(p, out);
    }
}

fn decode(data: &Value) -> String {
    let bytes = BASE64URL
        .decode(data.as_str().unwrap_or_default())
        .unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The address in `Name <address>`, or the whole value if there are no angle brackets.
fn address_of(mailbox: &str) -> &str {
    match (mailbox.rfind('<'), mailbox.rfind('>')) {
        (Some(start), Some(end)) if start < end => mailbox[start + 1..end].trim(),
        _ => mailbox.trim(),
    }
}

/// A plain address: something before and after one `@`, and nothing that could start another
/// address or header.
fn check_address(address: &str) -> Result<()> {
    let ok = address.split('@').count() == 2
        && !address.starts_with('@')
        && !address.ends_with('@')
        && !address
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "<>,;:\"()[]\\".contains(c));
    if !ok {
        bail!("invalid email address {address:?}: give plain addresses like user@example.com");
    }
    Ok(())
}

/// An RFC 5322 message: the headers, then the text, or a text and an HTML alternative.
fn mime_message(
    headers: &[(&str, String)],
    text: Option<&str>,
    html: Option<&str>,
) -> Result<String> {
    let mut out = String::new();
    for (name, value) in headers {
        if value.contains(['\r', '\n']) {
            bail!("{name} must be a single line");
        }
        out.push_str(&format!("{name}: {}\r\n", encode_header(value)));
    }
    out.push_str("MIME-Version: 1.0\r\n");
    match html {
        None => out.push_str(&text_part("text/plain", text.unwrap_or_default())),
        Some(html) => {
            let plain = match text {
                Some(t) => t.to_string(),
                None => html_to_text(html.as_bytes())?,
            };
            let boundary = format!("cedar-{}", uuid::Uuid::new_v4().simple());
            out.push_str(&format!(
                "Content-Type: multipart/alternative; boundary=\"{boundary}\"\r\n\r\n\
                 --{boundary}\r\n{}--{boundary}\r\n{}--{boundary}--\r\n",
                text_part("text/plain", &plain),
                text_part("text/html", html),
            ));
        }
    }
    Ok(out)
}

/// A UTF-8 body part, base64-encoded so no line length or character can break the message.
fn text_part(media_type: &str, content: &str) -> String {
    let encoded = STANDARD.encode(content);
    let mut lines = String::new();
    for chunk in encoded.as_bytes().chunks(76) {
        lines.push_str(std::str::from_utf8(chunk).expect("base64 is ASCII"));
        lines.push_str("\r\n");
    }
    format!(
        "Content-Type: {media_type}; charset=UTF-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{lines}"
    )
}

/// A header value, as RFC 2047 encoded words if it isn't plain ASCII.
fn encode_header(value: &str) -> String {
    if value.is_ascii() {
        return value.to_string();
    }
    // Encoded words may be at most 75 characters; 45 bytes of UTF-8 encode to 60.
    let mut words = Vec::new();
    let mut chunk = String::new();
    for c in value.chars() {
        if chunk.len() + c.len_utf8() > 45 {
            words.push(std::mem::take(&mut chunk));
        }
        chunk.push(c);
    }
    words.push(chunk);
    words
        .iter()
        .map(|w| format!("=?UTF-8?B?{}?=", STANDARD.encode(w)))
        .collect::<Vec<_>>()
        .join("\r\n ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(s: &str) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s)
    }

    #[test]
    fn the_plain_text_part_is_preferred_and_attachments_are_listed() {
        let msg = json!({ "id": "m1", "threadId": "t1", "labelIds": ["INBOX", "UNREAD"], "payload": {
            "mimeType": "multipart/mixed",
            "headers": [{ "name": "From", "value": "Alice <alice@example.com>" },
                        { "name": "subject", "value": "Lunch" }],
            "parts": [
                { "mimeType": "multipart/alternative", "parts": [
                    { "mimeType": "text/html", "body": { "data": b64("<p>Hi <b>there</b></p>") } },
                    { "mimeType": "text/plain", "body": { "data": b64("Hi there\n") } },
                ] },
                { "mimeType": "text/plain", "filename": "notes.txt", "body": { "size": 12, "attachmentId": "a" } },
            ] } });
        let rendered = without_empty(render_message(&msg, usize::MAX));
        assert_eq!(rendered["from"], "Alice <alice@example.com>");
        assert_eq!(rendered["subject"], "Lunch");
        assert_eq!(rendered["body"], "Hi there");
        assert_eq!(
            rendered["attachments"],
            json!([{ "filename": "notes.txt", "mimeType": "text/plain", "size": 12 }])
        );
        assert!(rendered.get("cc").is_none());
    }

    #[test]
    fn html_only_messages_are_converted_to_text() {
        let payload = json!({ "mimeType": "text/html", "body": { "data": b64("<p>Hello <b>world</b></p>") } });
        assert_eq!(body_text(&payload), "Hello **world**");

        let blank_plain = json!({ "mimeType": "multipart/alternative", "parts": [
            { "mimeType": "text/plain", "body": { "data": b64("\r\n\r\n") } },
            { "mimeType": "text/html", "body": { "data": b64("<p>The real text</p>") } },
        ] });
        assert_eq!(body_text(&blank_plain), "The real text");
    }

    #[test]
    fn addresses_are_checked() {
        assert!(check_address("a.b+c@example.com").is_ok());
        for bad in [
            "a",
            "@x.com",
            "a@",
            "a@b@c",
            "a@x.com\r\nBcc: e@x.com",
            "A <a@x.com>",
            "a@x.com,b@x.com",
        ] {
            assert!(check_address(bad).is_err(), "{bad}");
        }
        assert_eq!(address_of("Alice <alice@example.com>"), "alice@example.com");
        assert_eq!(address_of(" bob@example.com "), "bob@example.com");
    }

    #[test]
    fn messages_are_built_with_encoded_headers_and_bodies() {
        let headers = [
            ("To", "a@x.com".to_string()),
            ("Subject", "Grüße".to_string()),
        ];
        let raw = mime_message(&headers, Some("Hallo"), None).unwrap();
        assert!(
            raw.starts_with("To: a@x.com\r\nSubject: =?UTF-8?B?R3LDvMOfZQ==?=\r\n"),
            "{raw}"
        );
        assert!(raw.contains("Content-Type: text/plain; charset=UTF-8\r\n"));
        assert!(raw.ends_with("\r\n\r\nSGFsbG8=\r\n"), "{raw}");

        let raw = mime_message(&headers, None, Some("<p>Hi</p>")).unwrap();
        assert!(raw.contains("multipart/alternative"));
        assert!(raw.contains(&STANDARD.encode("<p>Hi</p>")));
        // Without a plain body, the HTML's text is the plain alternative.
        let plain = html_to_text(b"<p>Hi</p>").unwrap();
        assert!(raw.contains(&STANDARD.encode(plain)), "{raw}");

        let bad = [("Subject", "a\r\nBcc: x@y.com".to_string())];
        assert!(mime_message(&bad, None, None).is_err());
    }

    #[test]
    fn long_non_ascii_headers_become_several_encoded_words() {
        let encoded = encode_header(&"ä".repeat(40));
        assert_eq!(encoded.matches("=?UTF-8?B?").count(), 2);
        assert!(encoded.split("\r\n ").all(|w| w.len() <= 75));
    }
}
