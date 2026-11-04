//! Google Drive through the REST API: finding the user's files and reading Google Docs, Slides
//! and text files, and saving any file into the workspace for the sandbox tools. Like the Gmail
//! and Calendar tools they run in the cedar process: the OAuth tokens stay here, and the sandbox
//! has no network.
//!
//! Only reading is implemented; nothing here creates, changes, shares, or deletes a file.
//! Spreadsheets have their own tools (see `sheets`); `read_document` reads a Sheet's first tab as
//! CSV and says so.

use super::google::{page_size, required_str, select, str_arg, without_empty, GoogleApi};
use super::{truncate_output, Tool, ToolContext};
use crate::config::GoogleApiConfig;
use crate::mcp::GoogleOAuth;
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// The tools implemented here, in the order they are offered.
const TOOLS: &[&str] = &["search_files", "get_file", "read_document", "download_file"];
/// The metadata asked for about a file.
const FILE_FIELDS: &str = "id,name,mimeType,size,createdTime,modifiedTime,owners(displayName,emailAddress),lastModifyingUser(displayName,emailAddress),webViewLink,parents,description,shared";
/// Largest text `read_document` fetches from Drive.
const MAX_READ_BYTES: usize = 8 * 1024 * 1024;
/// Largest file `download_file` saves (Google's own export limit is 10 MB).
const MAX_DOWNLOAD_BYTES: usize = 25 * 1024 * 1024;
/// How much of a document `read_document` returns at once unless asked otherwise.
const DEFAULT_MAX_CHARS: usize = 40_000;
const MAX_CHARS: usize = 200_000;

const MIME_DOCUMENT: &str = "application/vnd.google-apps.document";
const MIME_SPREADSHEET: &str = "application/vnd.google-apps.spreadsheet";
const MIME_PRESENTATION: &str = "application/vnd.google-apps.presentation";
const MIME_FOLDER: &str = "application/vnd.google-apps.folder";
const MIME_SHORTCUT: &str = "application/vnd.google-apps.shortcut";

/// The Drive tools `cfg` allows, sharing one API client.
pub fn tools(cfg: &GoogleApiConfig, auth: Arc<GoogleOAuth>) -> Result<Vec<Box<dyn Tool>>> {
    let api = Arc::new(GoogleApi::new(&cfg.url, auth, "Drive")?);
    Ok(select(&cfg.prefix, &cfg.tools, TOOLS)
        .into_iter()
        .map(|op| {
            Box::new(DriveTool {
                name: format!("{}_{op}", cfg.prefix),
                op,
                api: api.clone(),
            }) as Box<dyn Tool>
        })
        .collect())
}

/// One Drive operation; `op` is one of [`TOOLS`].
pub struct DriveTool {
    name: String,
    op: &'static str,
    api: Arc<GoogleApi>,
}

#[async_trait]
impl Tool for DriveTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        match self.op {
            "search_files" => {
                "Find files in the user's Google Drive (their own and ones shared with them) by \
                 words in the name or the content, by kind (Google Doc, Sheet, Slides, folder, \
                 PDF, ...), folder and modification date, newest first. Returns each file's ID, \
                 which the other drive_ and sheets_ tools take."
            }
            "get_file" => {
                "Details of one Drive file by ID: name, kind, size, owner, who last changed it \
                 and when, its folder and link."
            }
            "read_document" => {
                "Read a file in the user's Google Drive as text: a Google Doc (as Markdown, with \
                 headings, lists and tables), Google Slides (the text of every slide), or a plain \
                 text, Markdown, CSV or JSON file. A Google Sheet gives its first tab as CSV; use \
                 the sheets_ tools for a particular tab or range. For a PDF, Word or Excel file use \
                 drive_download_file and then read_document. Long documents come in parts: the \
                 result says the total length and where to continue with offset."
            }
            "download_file" => {
                "Save a Drive file into the workspace so the sandbox tools can work on it: a \
                 Google Doc as docx, PDF, Markdown or text, a Sheet as xlsx or CSV, Slides as \
                 pptx or PDF, any other file as it is (up to 25 MB). Then use read_document, \
                 run_python, or the other sandbox tools on the saved file."
            }
            _ => unreachable!("not a Drive tool: {}", self.op),
        }
    }

    fn schema(&self) -> Value {
        let file_id =
            json!({ "type": "string", "description": "The file's ID, from drive_search_files." });
        match self.op {
            "search_files" => json!({ "type": "object", "properties": {
                "query": { "type": "string",
                    "description": "Words to look for in the files' content (and names)." },
                "name": { "type": "string",
                    "description": "Words the file name contains." },
                "kind": { "type": "string",
                    "enum": ["any", "document", "spreadsheet", "presentation", "folder", "pdf", "image"],
                    "description": "Only files of this kind (default any)." },
                "folderId": { "type": "string",
                    "description": "Only files directly in this folder (a folder's ID from an earlier search)." },
                "modifiedAfter": { "type": "string",
                    "description": "Only files changed after this time, ISO 8601 (e.g. 2026-04-01 or 2026-04-01T09:00:00Z)." },
                "pageSize": { "type": "integer", "minimum": 1, "maximum": 100,
                    "description": "How many to return (default 20, at most 100)." },
                "pageToken": { "type": "string",
                    "description": "The nextPageToken of a previous call, to get the next page." },
            } }),
            "get_file" => json!({ "type": "object", "properties": {
                "fileId": file_id,
            }, "required": ["fileId"] }),
            "read_document" => json!({ "type": "object", "properties": {
                "fileId": file_id,
                "format": { "type": "string", "enum": ["markdown", "text"],
                    "description": "How to render a Google Doc (default markdown)." },
                "offset": { "type": "integer", "minimum": 0,
                    "description": "Start this many characters into the document (default 0), to read the next part of a long one." },
                "maxChars": { "type": "integer", "minimum": 1000, "maximum": MAX_CHARS,
                    "description": format!("How many characters to return (default {DEFAULT_MAX_CHARS}).") },
            }, "required": ["fileId"] }),
            "download_file" => json!({ "type": "object", "properties": {
                "fileId": file_id,
                "path": { "type": "string",
                    "description": "Where to save the file, relative to /workspace. Parent directories are created." },
                "format": { "type": "string",
                    "enum": ["docx", "pdf", "markdown", "txt", "html", "xlsx", "csv", "ods", "pptx"],
                    "description": "For a Google Doc, Sheet or Slides file: what to export it as (default docx, xlsx, pptx). Ignored for other files, which are saved as they are." },
            }, "required": ["fileId", "path"] }),
            _ => unreachable!("not a Drive tool: {}", self.op),
        }
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let t = ctx.timeout;
        let out = match self.op {
            "search_files" => self.search_files(input, t).await?,
            "get_file" => {
                let id = required_str(input, "fileId")?;
                json!({ "file": render_file(&self.metadata(id, t).await?) })
            }
            "read_document" => self.read_document(input, ctx.max_output, t).await?,
            "download_file" => return self.download_file(ctx, input).await,
            _ => unreachable!("not a Drive tool: {}", self.op),
        };
        Ok(truncate_output(
            &serde_json::to_string(&without_empty(out))?,
            ctx.max_output,
        ))
    }
}

impl DriveTool {
    async fn metadata(&self, id: &str, t: Duration) -> Result<Value> {
        self.api
            .get(
                &["files", id],
                &[
                    ("fields", FILE_FIELDS.to_string()),
                    ("supportsAllDrives", "true".to_string()),
                ],
                t,
            )
            .await
    }

    async fn search_files(&self, input: &Value, t: Duration) -> Result<Value> {
        let mut terms = vec!["trashed = false".to_string()];
        let full_text = str_arg(input, "query");
        if let Some(words) = full_text {
            terms.push(format!("fullText contains '{}'", escape(words)));
        }
        if let Some(name) = str_arg(input, "name") {
            terms.push(format!("name contains '{}'", escape(name)));
        }
        match str_arg(input, "kind").unwrap_or("any") {
            "any" => {}
            "document" => terms.push(format!("mimeType = '{MIME_DOCUMENT}'")),
            "spreadsheet" => terms.push(format!("mimeType = '{MIME_SPREADSHEET}'")),
            "presentation" => terms.push(format!("mimeType = '{MIME_PRESENTATION}'")),
            "folder" => terms.push(format!("mimeType = '{MIME_FOLDER}'")),
            "pdf" => terms.push("mimeType = 'application/pdf'".to_string()),
            "image" => terms.push("mimeType contains 'image/'".to_string()),
            other => bail!("unknown kind {other:?}"),
        }
        if let Some(folder) = str_arg(input, "folderId") {
            terms.push(format!("'{}' in parents", escape(folder)));
        }
        if let Some(after) = str_arg(input, "modifiedAfter") {
            terms.push(format!(
                "modifiedTime > '{}'",
                escape(&normalize_time(after))
            ));
        }
        let mut query = vec![
            ("q", terms.join(" and ")),
            ("pageSize", page_size(input, 20, 100).to_string()),
            ("fields", format!("nextPageToken,files({FILE_FIELDS})")),
            ("supportsAllDrives", "true".to_string()),
            ("includeItemsFromAllDrives", "true".to_string()),
        ];
        // Drive ranks full-text matches by relevance; ordering by date would hide that.
        if full_text.is_none() {
            query.push(("orderBy", "modifiedTime desc".to_string()));
        }
        if let Some(token) = str_arg(input, "pageToken") {
            query.push(("pageToken", token.to_string()));
        }
        let list = self.api.get(&["files"], &query, t).await?;
        let files: Vec<Value> = list["files"]
            .as_array()
            .map(|a| a.iter().map(render_file).collect())
            .unwrap_or_default();
        Ok(json!({ "files": files, "nextPageToken": list["nextPageToken"] }))
    }

    async fn read_document(&self, input: &Value, max_output: usize, t: Duration) -> Result<Value> {
        let id = required_str(input, "fileId")?;
        let file = self.metadata(id, t).await?;
        let mime = file["mimeType"].as_str().unwrap_or_default().to_string();
        let name = file["name"].as_str().unwrap_or("the file");
        let mut note = None;
        let (text, format) = match mime.as_str() {
            MIME_DOCUMENT => match str_arg(input, "format") {
                Some("text") => (self.export(id, "text/plain", t).await?, "text"),
                _ => match self.export(id, "text/markdown", t).await {
                    Ok(text) => (text, "markdown"),
                    // Older deployments of the API only export plain text.
                    Err(_) => (self.export(id, "text/plain", t).await?, "text"),
                },
            },
            MIME_PRESENTATION => (self.export(id, "text/plain", t).await?, "text"),
            MIME_SPREADSHEET => {
                note = Some(
                    "This is the first tab only, as CSV. Use sheets_get_spreadsheet for the tabs \
                     and sheets_read_range for a tab or a range.",
                );
                (self.export(id, "text/csv", t).await?, "csv")
            }
            MIME_FOLDER => {
                bail!("{name} is a folder; search in it with drive_search_files and folderId")
            }
            MIME_SHORTCUT => bail!("{name} is a shortcut; use the file it points to"),
            m if m.starts_with("application/vnd.google-apps.") => {
                bail!("{name} is a {} and can't be read as text", kind_of(m))
            }
            m if is_text(m) => {
                let (bytes, _) = self
                    .api
                    .get_bytes(
                        &["files", id],
                        &[
                            ("alt", "media".to_string()),
                            ("supportsAllDrives", "true".to_string()),
                        ],
                        MAX_READ_BYTES,
                        t,
                    )
                    .await?;
                (String::from_utf8_lossy(&bytes).into_owned(), "text")
            }
            m => bail!(
                "{name} is {}, not text; save it with drive_download_file and then use \
                 read_document on the saved file",
                kind_of(m)
            ),
        };
        let total = text.chars().count();
        let offset = input["offset"].as_u64().unwrap_or(0) as usize;
        if offset > 0 && offset >= total {
            bail!("offset {offset} is past the end of the document ({total} characters)");
        }
        let wanted = (input["maxChars"]
            .as_u64()
            .unwrap_or(DEFAULT_MAX_CHARS as u64) as usize)
            .clamp(1000, MAX_CHARS);
        // The whole result must fit the output cap, with room for the metadata around the text.
        let fit = max_output.saturating_sub(2000).max(1000);
        let max_chars = wanted.min(fit);
        let content: String = text.chars().skip(offset).take(max_chars).collect();
        let end = offset + content.chars().count();
        let truncated = end < total;
        Ok(json!({
            "file": render_file(&file),
            "format": format,
            "content": content,
            "offset": offset,
            "totalChars": total,
            "truncated": truncated,
            "nextOffset": truncated.then_some(end),
            "note": note,
        }))
    }

    /// A Google Doc, Sheet or Slides file rendered as `mime_type`.
    async fn export(&self, id: &str, mime_type: &str, t: Duration) -> Result<String> {
        let (bytes, _) = self
            .api
            .get_bytes(
                &["files", id, "export"],
                &[("mimeType", mime_type.to_string())],
                MAX_READ_BYTES,
                t,
            )
            .await?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    async fn download_file(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let t = ctx.timeout;
        let id = required_str(input, "fileId")?;
        let path = required_str(input, "path")?;
        if path.contains('\0') {
            bail!("path must not contain NUL bytes");
        }
        if path.ends_with('/') {
            bail!("path must name a file, not a directory");
        }
        let file = self.metadata(id, t).await?;
        let mime = file["mimeType"].as_str().unwrap_or_default();
        let name = file["name"].as_str().unwrap_or("the file");
        let format = str_arg(input, "format");
        let (bytes, content_type) = if mime.starts_with("application/vnd.google-apps.") {
            let export_as = export_mime(mime, format).ok_or_else(|| match format {
                Some(f) => anyhow!("a {} can't be exported as {f}", kind_of(mime)),
                None => anyhow!("{name} is a {} and can't be downloaded", kind_of(mime)),
            })?;
            self.api
                .get_bytes(
                    &["files", id, "export"],
                    &[("mimeType", export_as.to_string())],
                    MAX_DOWNLOAD_BYTES,
                    t,
                )
                .await?
        } else {
            self.api
                .get_bytes(
                    &["files", id],
                    &[
                        ("alt", "media".to_string()),
                        ("supportsAllDrives", "true".to_string()),
                    ],
                    MAX_DOWNLOAD_BYTES,
                    t,
                )
                .await?
        };
        ctx.sandbox
            .write_file(path, &bytes)
            .await
            .with_context(|| format!("saving {path}"))?;
        let media = content_type.split(';').next().unwrap_or_default().trim();
        let mut msg = format!("saved {name} to {path}: {} bytes", bytes.len());
        if !media.is_empty() {
            msg.push_str(&format!(", {media}"));
        }
        Ok(msg)
    }
}

/// What a Google Doc, Sheet or Slides file is exported as: the `format` asked for, else the
/// Office format of its kind. `None` when the kind can't be exported that way.
fn export_mime(mime: &str, format: Option<&str>) -> Option<&'static str> {
    match (mime, format) {
        (MIME_DOCUMENT, None | Some("docx")) => {
            Some("application/vnd.openxmlformats-officedocument.wordprocessingml.document")
        }
        (MIME_DOCUMENT, Some("pdf")) => Some("application/pdf"),
        (MIME_DOCUMENT, Some("markdown")) => Some("text/markdown"),
        (MIME_DOCUMENT, Some("txt")) => Some("text/plain"),
        (MIME_DOCUMENT, Some("html")) => Some("text/html"),
        (MIME_SPREADSHEET, None | Some("xlsx")) => {
            Some("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
        }
        (MIME_SPREADSHEET, Some("csv")) => Some("text/csv"),
        (MIME_SPREADSHEET, Some("pdf")) => Some("application/pdf"),
        (MIME_SPREADSHEET, Some("ods")) => Some("application/vnd.oasis.opendocument.spreadsheet"),
        (MIME_PRESENTATION, None | Some("pptx")) => {
            Some("application/vnd.openxmlformats-officedocument.presentationml.presentation")
        }
        (MIME_PRESENTATION, Some("pdf")) => Some("application/pdf"),
        (MIME_PRESENTATION, Some("txt")) => Some("text/plain"),
        _ => None,
    }
}

/// A file's kind for people: "Google Doc", "folder", "PDF", else its media type.
fn kind_of(mime: &str) -> &str {
    match mime {
        MIME_DOCUMENT => "Google Doc",
        MIME_SPREADSHEET => "Google Sheet",
        MIME_PRESENTATION => "Google Slides",
        MIME_FOLDER => "folder",
        MIME_SHORTCUT => "shortcut",
        "application/vnd.google-apps.form" => "Google Form",
        "application/vnd.google-apps.drawing" => "Google Drawing",
        "application/pdf" => "PDF",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            "Word document"
        }
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "Excel workbook",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
            "PowerPoint presentation"
        }
        other => other,
    }
}

/// Whether a stored (non-Google) file can be read as text.
fn is_text(mime: &str) -> bool {
    let mime = mime.split(';').next().unwrap_or_default().trim();
    mime.starts_with("text/")
        || matches!(
            mime,
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "application/x-yaml"
                | "application/yaml"
                | "application/toml"
                | "application/x-sh"
        )
}

/// A value inside single quotes of a Drive query.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// `modifiedTime` comparisons need RFC 3339; a bare date means its midnight (UTC).
fn normalize_time(s: &str) -> String {
    if s.len() == 10 && s.chars().filter(|c| *c == '-').count() == 2 {
        format!("{s}T00:00:00Z")
    } else {
        s.to_string()
    }
}

/// A file as the model sees it.
fn render_file(f: &Value) -> Value {
    let mime = f["mimeType"].as_str().unwrap_or_default();
    let person = |p: &Value| match (p["displayName"].as_str(), p["emailAddress"].as_str()) {
        (Some(n), Some(e)) => Some(format!("{n} <{e}>")),
        (Some(n), None) => Some(n.to_string()),
        (None, Some(e)) => Some(e.to_string()),
        (None, None) => None,
    };
    json!({
        "id": f["id"],
        "name": f["name"],
        "kind": kind_of(mime),
        "mimeType": mime,
        "size": f["size"].as_str().and_then(|s| s.parse::<u64>().ok()),
        "createdTime": f["createdTime"],
        "modifiedTime": f["modifiedTime"],
        "owner": f["owners"].as_array().and_then(|o| o.first()).and_then(person),
        "lastModifiedBy": person(&f["lastModifyingUser"]),
        "folderId": f["parents"].as_array().and_then(|p| p.first()).cloned(),
        "link": f["webViewLink"],
        "description": f["description"],
        "shared": f["shared"].as_bool().filter(|s| *s),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_values_are_escaped() {
        assert_eq!(escape(r"it's a \ test"), r"it\'s a \\ test");
    }

    #[test]
    fn dates_become_timestamps() {
        assert_eq!(normalize_time("2026-04-01"), "2026-04-01T00:00:00Z");
        assert_eq!(
            normalize_time("2026-04-01T09:00:00Z"),
            "2026-04-01T09:00:00Z"
        );
    }

    #[test]
    fn export_formats_follow_the_kind() {
        assert!(export_mime(MIME_DOCUMENT, None)
            .unwrap()
            .contains("wordprocessingml"));
        assert_eq!(export_mime(MIME_SPREADSHEET, Some("csv")), Some("text/csv"));
        assert_eq!(export_mime(MIME_SPREADSHEET, Some("docx")), None);
        assert_eq!(export_mime(MIME_FOLDER, None), None);
    }

    #[test]
    fn files_are_rendered_for_the_model() {
        let f = render_file(&json!({
            "id": "f1", "name": "Plan", "mimeType": MIME_DOCUMENT, "size": "1234",
            "owners": [{ "displayName": "Ana", "emailAddress": "ana@example.com" }],
            "parents": ["folder-1"], "shared": false,
        }));
        assert_eq!(f["kind"], "Google Doc");
        assert_eq!(f["size"], 1234);
        assert_eq!(f["owner"], "Ana <ana@example.com>");
        assert_eq!(f["folderId"], "folder-1");
        assert!(f["shared"].is_null());
        assert!(is_text("text/csv; charset=utf-8"));
        assert!(!is_text("application/pdf"));
    }
}
