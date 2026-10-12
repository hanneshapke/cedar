//! `generate_pdf`: turns Markdown or HTML into a PDF file in the workspace.
//!
//! The rendering happens inside the sandbox, through the `cedar-render-pdf` command that the
//! sandbox image ships (`sandbox/render_pdf.py`, built on WeasyPrint). The document is staged in
//! the container's `/tmp`, rendered, and the staging file removed, so nothing but the PDF is left
//! in the workspace.

use super::{shell_quote, truncate_with_total, Tool, ToolContext};
use crate::sandbox::ExecOutput;
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;

/// Largest document `generate_pdf` accepts, matching `write_file`.
const MAX_CONTENT_BYTES: usize = 4 * 1024 * 1024;
/// The renderer command in the sandbox image.
const RENDERER: &str = "cedar-render-pdf";
const FORMATS: [&str; 2] = ["markdown", "html"];

pub struct GeneratePdf;

#[async_trait]
impl Tool for GeneratePdf {
    fn name(&self) -> &str {
        "generate_pdf"
    }

    fn description(&self) -> &str {
        "Create a PDF file in the sandbox from Markdown (headings, lists, tables, code blocks, \
         links, images from workspace files) or HTML. Use it when the user wants a document or \
         report as a PDF; they can download the file from the workspace afterwards. Page size, \
         fonts, and margins are chosen for you unless the HTML brings its own styles."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "path": { "type": "string",
                              "description": "Where to write the PDF, relative to /workspace. Must end in .pdf." },
                    "content": { "type": "string",
                                 "description": "The document, as Markdown (default) or HTML." },
                    "format": { "type": "string", "enum": FORMATS,
                                "description": "How to read `content` (default markdown)." },
                    "title": { "type": "string",
                               "description": "Document title for the PDF metadata. Defaults to the first heading." }
                },
                "required": ["path", "content"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let doc = parse_input(input)?;
        let staged = format!("/tmp/{RENDERER}-{}.{}", Uuid::new_v4(), doc.extension());
        ctx.sandbox
            .write_file(&staged, doc.content.as_bytes())
            .await
            .context("staging the document in the sandbox")?;
        let out = ctx
            .sandbox
            .exec(&render_command(&doc, &staged), ctx.timeout, ctx.max_output)
            .await?;
        report(doc.path, &out, ctx.timeout, ctx.max_output)
    }
}

#[derive(Debug, PartialEq)]
struct Document<'a> {
    path: &'a str,
    content: &'a str,
    format: &'a str,
    title: Option<&'a str>,
}

impl Document<'_> {
    fn extension(&self) -> &'static str {
        if self.format == "html" {
            "html"
        } else {
            "md"
        }
    }
}

fn parse_input(input: &Value) -> Result<Document<'_>> {
    let path = input["path"]
        .as_str()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| anyhow!("missing path"))?;
    if path.contains('\0') {
        bail!("path must not contain NUL bytes");
    }
    if !path.to_ascii_lowercase().ends_with(".pdf") {
        bail!("path must end in .pdf");
    }
    let content = input["content"]
        .as_str()
        .ok_or_else(|| anyhow!("missing content"))?;
    if content.trim().is_empty() {
        bail!("content is empty");
    }
    if content.len() > MAX_CONTENT_BYTES {
        bail!(
            "content is {} bytes; the limit is {MAX_CONTENT_BYTES}",
            content.len()
        );
    }
    let format = match input["format"].as_str() {
        None => "markdown",
        Some(f) if FORMATS.contains(&f) => f,
        Some(f) => bail!("invalid format {f:?}; use markdown or html"),
    };
    let title = input["title"]
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty());
    Ok(Document {
        path,
        content,
        format,
        title,
    })
}

/// Render `staged` to the document's path, then remove the staging file whatever happened,
/// keeping the renderer's exit status.
fn render_command(doc: &Document<'_>, staged: &str) -> String {
    let mut cmd = format!("{RENDERER} --format {}", doc.format);
    if let Some(title) = doc.title {
        cmd.push_str(" --title ");
        cmd.push_str(&shell_quote(title));
    }
    let staged = shell_quote(staged);
    cmd.push_str(&format!(
        " -- {staged} {}; s=$?; rm -f -- {staged}; exit $s",
        shell_quote(doc.path)
    ));
    cmd
}

/// Turn the renderer's exit status and output into the tool result: a one-line summary (with
/// the renderer's warnings, if any) on success, an error the model can act on otherwise.
fn report(path: &str, out: &ExecOutput, timeout: Duration, max_output: usize) -> Result<String> {
    let stderr = truncate_with_total(
        &String::from_utf8_lossy(&out.stderr),
        out.stderr_total,
        max_output,
    );
    let stderr = stderr.trim();
    match out.exit_code {
        Some(0) => {}
        Some(124) => bail!(
            "rendering {path} timed out after {}s; try a shorter document",
            timeout.as_secs()
        ),
        Some(127) => bail!(
            "the sandbox image has no `{RENDERER}` command; rebuild it from sandbox/Dockerfile ({stderr})"
        ),
        Some(code) => bail!("rendering {path} failed (exit status {code}): {stderr}"),
        None => bail!("rendering {path} was terminated by a signal"),
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stats: Option<Value> = stdout
        .lines()
        .last()
        .and_then(|l| serde_json::from_str(l).ok());
    let mut msg = match stats {
        Some(s) if s["pages"].is_u64() && s["bytes"].is_u64() => format!(
            "wrote {path}: {} page{}, {} bytes",
            s["pages"],
            if s["pages"] == 1 { "" } else { "s" },
            s["bytes"]
        ),
        _ => format!("wrote {path}"),
    };
    if !stderr.is_empty() {
        msg.push_str("\n[warnings]\n");
        msg.push_str(stderr);
    }
    Ok(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exec(code: Option<i32>, stdout: &str, stderr: &str) -> ExecOutput {
        ExecOutput {
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
            stdout_total: stdout.len(),
            stderr_total: stderr.len(),
            exit_code: code,
        }
    }

    const TIMEOUT: Duration = Duration::from_secs(60);

    #[test]
    fn input_defaults_to_markdown_without_a_title() {
        let input = json!({ "path": " out/report.pdf ", "content": "# Hi" });
        assert_eq!(
            parse_input(&input).unwrap(),
            Document {
                path: "out/report.pdf",
                content: "# Hi",
                format: "markdown",
                title: None
            }
        );
        let input =
            json!({ "path": "a.PDF", "content": "<p>x</p>", "format": "html", "title": " T " });
        let doc = parse_input(&input).unwrap();
        assert_eq!(
            (doc.format, doc.title, doc.extension()),
            ("html", Some("T"), "html")
        );
    }

    #[test]
    fn bad_input_is_rejected() {
        let err = |v: Value| parse_input(&v).unwrap_err().to_string();
        assert_eq!(err(json!({ "content": "x" })), "missing path");
        assert_eq!(err(json!({ "path": "  ", "content": "x" })), "missing path");
        assert_eq!(
            err(json!({ "path": "a.txt", "content": "x" })),
            "path must end in .pdf"
        );
        assert_eq!(err(json!({ "path": "a.pdf" })), "missing content");
        assert_eq!(
            err(json!({ "path": "a.pdf", "content": " \n" })),
            "content is empty"
        );
        assert!(
            err(json!({ "path": "a.pdf", "content": "x", "format": "docx" })).contains("format")
        );
        assert!(err(json!({ "path": "a\0.pdf", "content": "x" })).contains("NUL"));
        let big = "x".repeat(MAX_CONTENT_BYTES + 1);
        assert!(err(json!({ "path": "a.pdf", "content": big })).contains("limit"));
    }

    #[test]
    fn the_command_renders_then_cleans_up_with_the_renderer_status() {
        let doc = Document {
            path: "out/it's.pdf",
            content: "# x",
            format: "markdown",
            title: Some("Q3 \"report\""),
        };
        assert_eq!(
            render_command(&doc, "/tmp/stage.md"),
            "cedar-render-pdf --format markdown --title 'Q3 \"report\"' -- '/tmp/stage.md' 'out/it'\\''s.pdf'; \
             s=$?; rm -f -- '/tmp/stage.md'; exit $s"
        );
        let doc = Document {
            title: None,
            format: "html",
            ..doc
        };
        assert!(
            render_command(&doc, "/tmp/s.html").starts_with("cedar-render-pdf --format html -- ")
        );
    }

    #[test]
    fn success_reports_pages_and_bytes_plus_warnings() {
        let out = exec(Some(0), "{\"pages\": 3, \"bytes\": 12345}\n", "");
        assert_eq!(
            report("r.pdf", &out, TIMEOUT, 1000).unwrap(),
            "wrote r.pdf: 3 pages, 12345 bytes"
        );
        let out = exec(
            Some(0),
            "{\"pages\": 1, \"bytes\": 9}\n",
            "Failed to load image at chart.png\n",
        );
        assert_eq!(
            report("r.pdf", &out, TIMEOUT, 1000).unwrap(),
            "wrote r.pdf: 1 page, 9 bytes\n[warnings]\nFailed to load image at chart.png"
        );
        // A renderer that prints something else still counts as success.
        assert_eq!(
            report("r.pdf", &exec(Some(0), "ok\n", ""), TIMEOUT, 1000).unwrap(),
            "wrote r.pdf"
        );
    }

    #[test]
    fn failures_become_tool_errors() {
        let err = |out: ExecOutput| {
            report("r.pdf", &out, TIMEOUT, 1000)
                .unwrap_err()
                .to_string()
        };
        assert_eq!(
            err(exec(Some(1), "", "rendering failed: bad css\n")),
            "rendering r.pdf failed (exit status 1): rendering failed: bad css"
        );
        assert!(
            err(exec(Some(127), "", "sh: 1: cedar-render-pdf: not found")).contains("rebuild it")
        );
        assert!(err(exec(Some(124), "", "")).contains("timed out after 60s"));
        assert!(err(exec(None, "", "")).contains("signal"));
    }

    #[test]
    fn long_stderr_is_truncated_in_the_error() {
        let out = ExecOutput {
            stderr: vec![b'e'; 50],
            stderr_total: 5000,
            exit_code: Some(1),
            ..Default::default()
        };
        let err = report("r.pdf", &out, TIMEOUT, 50).unwrap_err().to_string();
        assert!(err.ends_with("[truncated 4950 bytes]"), "{err}");
    }
}
