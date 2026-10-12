//! `read_document`: extracts the text of a PDF, Word, Excel, PowerPoint, CSV, or HTML file in the
//! workspace.
//!
//! The extraction happens inside the sandbox, through the `cedar-read-document` command that the
//! sandbox image ships (`sandbox/read_document.py`, built on pypdf, python-docx, openpyxl, and
//! python-pptx).

use super::{shell_quote, stderr_text, truncate_with_total, Tool, ToolContext};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use serde_json::{json, Value};

/// The extractor command in the sandbox image.
const READER: &str = "cedar-read-document";

pub struct ReadDocument;

#[async_trait]
impl Tool for ReadDocument {
    fn name(&self) -> &str {
        "read_document"
    }

    fn description(&self) -> &str {
        "Extract the text of a document in the workspace: PDF (.pdf), Word (.docx), Excel \
         (.xlsx, .xlsm), PowerPoint (.pptx), CSV/TSV, or HTML. Tables come back as Markdown tables. \
         Use `pages` to read part of a long PDF or deck, and `sheet` and `rows` for a large \
         spreadsheet or CSV. For plain text files, use read_file."
    }

    fn schema(&self) -> Value {
        json!({ "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "The document, relative to /workspace." },
                    "pages": { "type": "string",
                               "description": "PDF pages or PowerPoint slides to read, 1-based: \"3\", \"2-5\", or \"1,4-6\". Default all." },
                    "sheet": { "type": "string",
                               "description": "Excel sheet to read. Default all sheets." },
                    "rows": { "type": "string",
                              "description": "Spreadsheet or CSV data rows to read, 1-based, like pages. The header row is always shown. Default all." }
                },
                "required": ["path"] })
    }

    async fn call(&self, ctx: &ToolContext, input: &Value) -> Result<String> {
        let req = parse_input(input)?;
        let out = ctx
            .sandbox
            .exec(&read_command(&req), ctx.timeout, ctx.max_output)
            .await?;
        let stderr = stderr_text(&out, ctx.max_output);
        match out.exit_code {
            Some(0) => {}
            Some(124) => bail!(
                "reading {} timed out after {}s; read fewer pages or rows",
                req.path,
                ctx.timeout.as_secs()
            ),
            Some(127) => bail!(
                "the sandbox image has no `{READER}` command; rebuild it from sandbox/Dockerfile ({stderr})"
            ),
            Some(_) => bail!("{stderr}"),
            None => bail!("reading {} was terminated by a signal", req.path),
        }
        let mut text = truncate_with_total(
            &String::from_utf8_lossy(&out.stdout),
            out.stdout_total,
            ctx.max_output,
        );
        if out.stdout_total > ctx.max_output {
            text.push_str("\n[the document is longer than the output limit; use pages, sheet, or rows to read the rest]");
        }
        if !stderr.is_empty() {
            text.push_str("\n[warnings]\n");
            text.push_str(&stderr);
        }
        Ok(text)
    }
}

#[derive(Debug, PartialEq)]
struct Request<'a> {
    path: &'a str,
    pages: Option<&'a str>,
    sheet: Option<&'a str>,
    rows: Option<&'a str>,
}

fn parse_input(input: &Value) -> Result<Request<'_>> {
    let path = input["path"]
        .as_str()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| anyhow!("missing path"))?;
    if path.contains('\0') {
        bail!("path must not contain NUL bytes");
    }
    let opt = |k: &str| input[k].as_str().map(str::trim).filter(|v| !v.is_empty());
    let pages = opt("pages");
    let rows = opt("rows");
    for (name, range) in [("pages", pages), ("rows", rows)] {
        if let Some(r) = range {
            check_range(r).map_err(|e| anyhow!("invalid {name} {r:?}: {e}"))?;
        }
    }
    Ok(Request {
        path,
        pages,
        sheet: opt("sheet"),
        rows,
    })
}

/// `N`, `N-M`, `N-` and comma-separated lists of those, with 1-based N <= M.
fn check_range(spec: &str) -> Result<()> {
    for part in spec.split(',').map(str::trim) {
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => (a.trim(), Some(b.trim())),
            None => (part, None),
        };
        let start: u64 = a
            .parse()
            .map_err(|_| anyhow!("use forms like 3, 2-5, or 1,4-6"))?;
        if start == 0 {
            bail!("numbers start at 1");
        }
        if let Some(b) = b.filter(|b| !b.is_empty()) {
            let end: u64 = b
                .parse()
                .map_err(|_| anyhow!("use forms like 3, 2-5, or 1,4-6"))?;
            if end < start {
                bail!("{start}-{end} is backwards");
            }
        }
    }
    Ok(())
}

fn read_command(req: &Request<'_>) -> String {
    let mut cmd = READER.to_string();
    for (flag, value) in [
        ("--pages", req.pages),
        ("--sheet", req.sheet),
        ("--rows", req.rows),
    ] {
        if let Some(v) = value {
            cmd.push_str(&format!(" {flag} {}", shell_quote(v)));
        }
    }
    cmd.push_str(" -- ");
    cmd.push_str(&shell_quote(req.path));
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_is_trimmed_and_optional_fields_dropped_when_blank() {
        let input = json!({ "path": " report.pdf ", "pages": "", "sheet": " Q3 " });
        assert_eq!(
            parse_input(&input).unwrap(),
            Request {
                path: "report.pdf",
                pages: None,
                sheet: Some("Q3"),
                rows: None
            }
        );
    }

    #[test]
    fn bad_input_is_rejected() {
        let err = |v: Value| parse_input(&v).unwrap_err().to_string();
        assert_eq!(err(json!({})), "missing path");
        assert!(err(json!({ "path": "a\0b" })).contains("NUL"));
        assert!(err(json!({ "path": "a.pdf", "pages": "x" })).contains("invalid pages"));
        assert!(err(json!({ "path": "a.pdf", "pages": "0" })).contains("start at 1"));
        assert!(err(json!({ "path": "a.csv", "rows": "5-2" })).contains("backwards"));
    }

    #[test]
    fn ranges_accept_single_spans_open_ends_and_lists() {
        for ok in ["3", "2-5", "1,4-6", "10-", " 1 - 2 , 7 "] {
            assert!(check_range(ok).is_ok(), "{ok}");
        }
        for bad in ["", "a", "1-b", "-3", "0-2", "1,,2"] {
            assert!(check_range(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_command_quotes_every_argument() {
        let req = Request {
            path: "it's.xlsx",
            pages: None,
            sheet: Some("Q3 $(x)"),
            rows: Some("1-10"),
        };
        assert_eq!(
            read_command(&req),
            "cedar-read-document --sheet 'Q3 $(x)' --rows '1-10' -- 'it'\\''s.xlsx'"
        );
        let req = Request {
            path: "a.pdf",
            pages: Some("2"),
            sheet: None,
            rows: None,
        };
        assert_eq!(
            read_command(&req),
            "cedar-read-document --pages '2' -- 'a.pdf'"
        );
    }
}
