//! Files sent with a prompt, made ready for the model when the run starts.
//!
//! Every attachment is copied into the workspace under `attachments/`, so the agent can work on
//! it with its tools (`read_document` for a PDF's text, `run_python`, ...). A model that takes
//! images also sees each image, and the first pages of each PDF rendered as images (by
//! `cedar-render-pages` in the sandbox, `sandbox/render_pages.py`). PDFs are not sent as files:
//! OpenAI-compatible servers such as vLLM take images in a prompt but not PDFs.
//!
//! A text note listing the saved files goes with the blocks, so the model knows where they are
//! and how much of each PDF it was shown.

use crate::models::Attachment;
use crate::sandbox::{Sandbox, SandboxFatal};
use crate::tools::shell_quote;
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::time::Duration;

/// Where attachments go, relative to /workspace.
pub const DIR: &str = "attachments";
/// The page renderer in the sandbox image.
const RENDERER: &str = "cedar-render-pages";
/// The longer side of a rendered page, in pixels: legible text without a huge image.
const PAGE_MAX_SIDE: u32 = 1568;
/// A rendered page larger than this is left out (it would be a pathological page).
const MAX_PAGE_IMAGE_BYTES: usize = 5 << 20;
/// Longest file name (in bytes) an attachment is saved under; file systems allow 255.
const MAX_NAME_BYTES: usize = 120;

/// Copy `attachments` into the sandbox's workspace and build the blocks the run's opening message
/// gets for them: image blocks, then the note. With `images` (the model takes them), images are
/// shown as they are and PDFs as up to `pdf_pages` page images in all. A PDF that can't be
/// rendered is still saved and listed; failing to save a file, or a broken sandbox, fails.
pub async fn prepare(
    sandbox: &dyn Sandbox,
    attachments: &[Attachment],
    images: bool,
    pdf_pages: usize,
    timeout: Duration,
) -> Result<Vec<Value>> {
    let mut blocks = Vec::new();
    let mut lines = Vec::new();
    let mut budget = pdf_pages;
    for (i, (a, name)) in attachments.iter().zip(file_names(attachments)).enumerate() {
        let path = format!("{DIR}/{name}");
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&a.data)
            .with_context(|| format!("attachment {name:?} is not valid base64"))?;
        sandbox
            .write_file(&path, &bytes)
            .await
            .with_context(|| format!("saving attachment {name:?} in the workspace"))?;

        let what = if a.is_image() {
            if images {
                blocks.push(a.to_block());
                "image, shown above".to_string()
            } else {
                "image; this model can't view images".to_string()
            }
        } else if !images || budget == 0 {
            "PDF; read its text with read_document".to_string()
        } else {
            let dir = format!("/tmp/cedar-pages-{i}");
            let rendered = render(sandbox, &path, &dir, budget, timeout).await;
            // Best effort: the container goes away with the run anyway.
            let _ = sandbox
                .exec(&format!("rm -rf -- {}", shell_quote(&dir)), timeout, 1024)
                .await;
            match rendered {
                Ok(pages) => {
                    budget -= pages.images.len();
                    for (n, data) in pages.images.iter().enumerate() {
                        blocks.push(page_block(&name, n + 1, data));
                    }
                    pdf_line(pages.page_count, pages.images.len())
                }
                Err(e) if e.is::<SandboxFatal>() => return Err(e),
                Err(e) => {
                    tracing::warn!(error = %format!("{e:#}"), attachment = %name, "could not render PDF pages");
                    format!("PDF; its pages could not be shown ({e:#}); read its text with read_document")
                }
            }
        };
        lines.push(format!("- {path}: {what}"));
    }
    if !lines.is_empty() {
        blocks.push(json!({ "type": "text", "text": note(&lines) }));
    }
    Ok(blocks)
}

fn note(lines: &[String]) -> String {
    format!(
        "[Files attached to this message, saved in the workspace:\n{}]\n\n",
        lines.join("\n")
    )
}

fn pdf_line(page_count: usize, shown: usize) -> String {
    let pages = if page_count == 1 { "page" } else { "pages" };
    if shown == 0 {
        format!("PDF, {page_count} {pages}; read its text with read_document")
    } else if shown >= page_count {
        format!("PDF, {page_count} {pages}, shown above as images; read_document gives its text")
    } else {
        let which = if shown == 1 {
            "page 1 is".to_string()
        } else {
            format!("pages 1-{shown} are")
        };
        format!(
            "PDF, {page_count} {pages}; {which} shown above as images. Use read_document for \
             its text, including the other pages"
        )
    }
}

fn page_block(name: &str, page: usize, data: &str) -> Value {
    json!({
        "type": "image",
        "source": { "type": "base64", "media_type": "image/jpeg", "data": data },
        "title": format!("{name} p.{page}"),
    })
}

struct Pages {
    page_count: usize,
    /// Base64 JPEGs, from page 1 on.
    images: Vec<String>,
}

#[derive(Deserialize)]
struct RenderReport {
    page_count: usize,
    images: Vec<String>,
}

/// Render up to `max` pages of the workspace PDF at `path` into `dir` and read them back.
async fn render(
    sandbox: &dyn Sandbox,
    path: &str,
    dir: &str,
    max: usize,
    timeout: Duration,
) -> Result<Pages> {
    let command = format!(
        "{RENDERER} --pages {max} --max-side {PAGE_MAX_SIDE} --out {} -- {}",
        shell_quote(dir),
        shell_quote(path)
    );
    let out = sandbox.exec(&command, timeout, 64 * 1024).await?;
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    match out.exit_code {
        Some(0) => {}
        Some(124) => bail!("rendering timed out after {}s", timeout.as_secs()),
        Some(127) => bail!(
            "the sandbox image has no `{RENDERER}` command; rebuild it from sandbox/Dockerfile"
        ),
        Some(_) if !stderr.is_empty() => bail!("{stderr}"),
        _ => bail!("the renderer failed"),
    }
    let report: RenderReport = serde_json::from_slice(&out.stdout)
        .map_err(|e| anyhow!("unexpected renderer output: {e}"))?;
    let mut images = Vec::with_capacity(report.images.len());
    for image in report.images.iter().take(max) {
        let file = sandbox.read_file(image, MAX_PAGE_IMAGE_BYTES).await?;
        if file.truncated {
            break; // later pages would be shown without this one
        }
        images.push(base64::engine::general_purpose::STANDARD.encode(&file.bytes));
    }
    Ok(Pages {
        page_count: report.page_count,
        images,
    })
}

/// The file name each attachment is saved under: its own name made safe for a single path
/// component (no separators, not hidden, not too long), with `-2`, `-3`, ... before the
/// extension when two attachments would get the same name.
pub fn file_names(attachments: &[Attachment]) -> Vec<String> {
    let mut taken = HashSet::new();
    attachments
        .iter()
        .map(|a| {
            let base = safe_name(&a.name);
            let (stem, ext) = match base.rsplit_once('.') {
                Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), format!(".{ext}")),
                _ => (base.clone(), String::new()),
            };
            let mut name = base;
            let mut n = 2;
            while !taken.insert(name.clone()) {
                name = format!("{stem}-{n}{ext}");
                n += 1;
            }
            name
        })
        .collect()
}

fn safe_name(name: &str) -> String {
    let mut s: String = name
        .trim()
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    if s.starts_with('.') {
        s.replace_range(..1, "_");
    }
    if s.len() > MAX_NAME_BYTES {
        // Keep the extension; cut the rest on a character boundary.
        let ext = s
            .rsplit_once('.')
            .map(|(_, e)| format!(".{e}"))
            .filter(|e| e.len() <= 16)
            .unwrap_or_default();
        let mut cut = MAX_NAME_BYTES - ext.len();
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s = format!("{}{ext}", &s[..cut]);
    }
    if s.is_empty() {
        s = "file".into();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn att(name: &str) -> Attachment {
        Attachment {
            name: name.into(),
            media_type: "application/pdf".into(),
            data: "JVBERi0=".into(),
        }
    }

    #[test]
    fn names_are_made_safe_and_unique() {
        let names = file_names(&[
            att("report.pdf"),
            att("report.pdf"),
            att("../../etc/passwd"),
            att(".hidden"),
            att("a\\b.png"),
            att("report.pdf"),
            att("noext"),
            att("noext"),
        ]);
        assert_eq!(
            names,
            [
                "report.pdf",
                "report-2.pdf",
                "_._.._etc_passwd",
                "_hidden",
                "a_b.png",
                "report-3.pdf",
                "noext",
                "noext-2",
            ]
        );
    }

    #[test]
    fn long_names_keep_their_extension() {
        let long = format!("{}.pdf", "ä".repeat(150));
        let name = &file_names(&[att(&long)])[0];
        assert!(name.len() <= MAX_NAME_BYTES, "{}", name.len());
        assert!(name.ends_with("ä.pdf"), "{name}");
    }

    #[test]
    fn pdf_lines_say_how_much_was_shown() {
        assert_eq!(
            pdf_line(1, 1),
            "PDF, 1 page, shown above as images; read_document gives its text"
        );
        assert!(pdf_line(48, 10).contains("pages 1-10 are shown above"));
        assert!(pdf_line(48, 1).contains("; page 1 is shown above"));
        assert!(pdf_line(3, 0).starts_with("PDF, 3 pages; read its text"));
    }
}
