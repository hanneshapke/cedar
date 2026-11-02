//! "Open in new tab" for a workspace file in the chat's file viewer. The web view can't show the
//! viewer's blob: URL in a tab of its own, and the system browser can't open one either, so the
//! page sends the window to `/__cedar-app/open-file?conversation=…&path=…` (see window.rs) and the
//! app fetches the file from cedar-chat with its access token, keeps it in a private temporary
//! folder and opens it in the default app. Only the kinds the viewer opens in a tab: raster images
//! and PDFs, each checked against its name, so a page can't have the app open anything else.

use crate::app::AppState;
use anyhow::{anyhow, bail, Context, Result};
use futures_util::StreamExt;
use rand::RngCore;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use tauri::{AppHandle, Manager};
use url::Url;

/// The viewer's own limit for images (file-viewer.js).
const MAX_IMAGE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PDF_BYTES: u64 = 64 * 1024 * 1024;
/// How long an opened file is kept: the default app reads it after the app has handed it over.
const KEEP: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_NAME_CHARS: usize = 80;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Png,
    Jpeg,
    Gif,
    Webp,
    Bmp,
    Avif,
    Pdf,
}

impl Kind {
    /// The kind a file name's extension says, or None for every other file (SVG among them: its
    /// scripts would run wherever it opens).
    pub fn from_name(name: &str) -> Option<Kind> {
        let (_, ext) = name.rsplit_once('.')?;
        Some(match ext.to_ascii_lowercase().as_str() {
            "png" => Kind::Png,
            "jpg" | "jpeg" => Kind::Jpeg,
            "gif" => Kind::Gif,
            "webp" => Kind::Webp,
            "bmp" => Kind::Bmp,
            "avif" => Kind::Avif,
            "pdf" => Kind::Pdf,
            _ => return None,
        })
    }

    fn extension(self) -> &'static str {
        match self {
            Kind::Png => "png",
            Kind::Jpeg => "jpg",
            Kind::Gif => "gif",
            Kind::Webp => "webp",
            Kind::Bmp => "bmp",
            Kind::Avif => "avif",
            Kind::Pdf => "pdf",
        }
    }

    fn max_bytes(self) -> u64 {
        if self == Kind::Pdf {
            MAX_PDF_BYTES
        } else {
            MAX_IMAGE_BYTES
        }
    }

    /// Whether `bytes` start like a file of this kind. The default app is picked by the name, so
    /// a .png holding HTML or a script never reaches it.
    fn matches(self, bytes: &[u8]) -> bool {
        match self {
            Kind::Png => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            Kind::Jpeg => bytes.starts_with(b"\xff\xd8\xff"),
            Kind::Gif => bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a"),
            Kind::Webp => bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP",
            Kind::Bmp => bytes.starts_with(b"BM"),
            Kind::Avif => bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && matches!(&bytes[8..12], b"avif" | b"avis"),
            // As the viewer checks it: "%PDF-" within the first KiB.
            Kind::Pdf => bytes[..bytes.len().min(1024)].windows(5).any(|w| w == b"%PDF-"),
        }
    }
}

/// A workspace file the chat page asked to open.
#[derive(Debug, PartialEq)]
pub struct OpenFile {
    pub conversation: String,
    /// Relative to the workspace, one name per element: no empty, `.` or `..` parts.
    pub path: Vec<String>,
    pub kind: Kind,
}

impl OpenFile {
    /// The request from the page's `conversation` and `path`, or None unless the conversation is
    /// an id and the path a plain workspace path to a file of a kind that opens.
    pub fn parse(conversation: &str, path: &str) -> Option<OpenFile> {
        let id_like = |c: char| c.is_ascii_hexdigit() || c == '-';
        if conversation.is_empty() || conversation.len() > 36 || !conversation.chars().all(id_like) {
            return None;
        }
        let parts: Vec<String> = path.split('/').map(str::to_owned).collect();
        let plain = |p: &String| {
            !p.is_empty() && p != "." && p != ".." && !p.contains(['\\', '\0', ':']) && !p.chars().any(char::is_control)
        };
        if !parts.iter().all(plain) {
            return None;
        }
        let kind = Kind::from_name(parts.last()?)?;
        Some(OpenFile { conversation: conversation.to_owned(), path: parts, kind })
    }

    /// cedar-chat's address for the file. Each part is percent-encoded, so it stays one segment.
    fn url(&self, chat: &str) -> Result<Url> {
        let mut url = Url::parse(chat)?;
        url.path_segments_mut()
            .map_err(|_| anyhow!("the chat's URL has no path"))?
            .clear()
            .extend(["api", "conversations", &self.conversation, "files"])
            .extend(&self.path);
        Ok(url)
    }

    /// A name that is safe on every file system: the file's own name with anything but letters,
    /// digits, spaces, `-`, `_` and `.` replaced, and the extension of its kind.
    fn file_name(&self) -> String {
        let last = self.path.last().map(String::as_str).unwrap_or_default();
        let stem = last.rsplit_once('.').map_or(last, |(stem, _)| stem);
        let stem: String = stem
            .chars()
            .map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.') { c } else { '_' })
            .take(MAX_NAME_CHARS)
            .collect();
        let stem = stem.trim_start_matches('.').trim();
        let stem = if stem.is_empty() { "file" } else { stem };
        format!("{stem}.{}", self.kind.extension())
    }
}

/// Fetches the file, saves it and opens it in the default app. Failures go to the log.
pub async fn open(app: AppHandle, file: OpenFile) {
    if let Err(e) = try_open(&app, &file).await {
        eprintln!("opening {} from the workspace: {e:#}", file.path.join("/"));
    }
}

async fn try_open(app: &AppHandle, file: &OpenFile) -> Result<()> {
    let cfg = app.state::<AppState>().config();
    let bytes = fetch(file, &cfg.chat_url(), &cfg.secrets.chat_access_token).await?;
    let saved = save(&std::env::temp_dir().join("cedar-open"), &file.file_name(), &bytes)?;
    open::that(&saved).with_context(|| format!("opening {}", saved.display()))
}

async fn fetch(file: &OpenFile, chat: &str, token: &str) -> Result<Vec<u8>> {
    let max = file.kind.max_bytes();
    let resp = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()?
        .get(file.url(chat)?)
        .bearer_auth(token)
        .send()
        .await
        .context("fetching the file")?
        .error_for_status()
        .context("fetching the file")?;
    if resp.content_length().is_some_and(|n| n > max) {
        bail!("the file is larger than {} MiB", max / 1024 / 1024);
    }
    let mut bytes = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        bytes.extend_from_slice(&chunk.context("fetching the file")?);
        if bytes.len() as u64 > max {
            bail!("the file is larger than {} MiB", max / 1024 / 1024);
        }
    }
    if !file.kind.matches(&bytes) {
        bail!("the file isn't the {} its name says", file.kind.extension());
    }
    Ok(bytes)
}

/// Writes `bytes` as `name` into a new folder of its own under `root`, which only this user can
/// read. A fresh folder per file keeps the name and never meets an existing file. Folders older
/// than [`KEEP`] go first.
fn save(root: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf> {
    std::fs::create_dir_all(root).with_context(|| format!("creating {}", root.display()))?;
    let meta = std::fs::symlink_metadata(root)?;
    if !meta.is_dir() {
        bail!("{} isn't a folder", root.display());
    }
    private(root, 0o700)?;
    prune(root);
    let mut id = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut id);
    let dir = root.join(hex::encode(id));
    std::fs::create_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
    private(&dir, 0o700)?;
    let path = dir.join(name);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut out = options.open(&path).with_context(|| format!("creating {}", path.display()))?;
    std::io::Write::write_all(&mut out, bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// Only this user may read the folder. On Windows the user's temporary folder already is private.
fn private(dir: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode))
            .with_context(|| format!("restricting {}", dir.display()))?;
    }
    #[cfg(not(unix))]
    let _ = (dir, mode);
    Ok(())
}

/// Removes the folders of files opened more than [`KEEP`] ago.
fn prune(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| now.duration_since(t).is_ok_and(|age| age > KEEP));
        if old && entry.file_type().is_ok_and(|t| t.is_dir()) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0c6f1b2e-8a4d-4f7e-9b1a-2d3c4e5f6a7b";

    #[test]
    fn only_plain_workspace_paths_to_images_and_pdfs_open() {
        assert!(OpenFile::parse(ID, "chart.png").is_some());
        assert!(OpenFile::parse(ID, "out/report.PDF").is_some());
        assert!(OpenFile::parse(ID, "photos/a b.JPEG").is_some());
        for path in [
            "../secret.png",
            "a/../../b.png",
            "./a.png",
            "/etc/a.png",
            "a//b.png",
            "a\\..\\b.png",
            "C:/a.png",
            "a\0.png",
            "a.png/",
            "",
            "drawing.svg",
            "page.html",
            "notes.md",
            "png",
        ] {
            assert_eq!(OpenFile::parse(ID, path), None, "{path:?}");
        }
    }

    #[test]
    fn the_conversation_must_be_an_id() {
        for id in ["", "../x", "abc/def", "0c6f1b2e?x=1", &"a".repeat(37)] {
            assert_eq!(OpenFile::parse(id, "a.png"), None, "{id:?}");
        }
    }

    #[test]
    fn the_file_is_fetched_from_its_workspace_with_each_name_encoded() {
        let f = OpenFile::parse(ID, "my dir/a#b?c%2F.png").unwrap();
        let url = f.url("http://127.0.0.1:13000").unwrap();
        assert_eq!(
            url.as_str(),
            format!("http://127.0.0.1:13000/api/conversations/{ID}/files/my%20dir/a%23b%3Fc%252F.png")
        );
        assert_eq!(url.query(), None);
    }

    #[test]
    fn saved_names_are_safe_and_keep_their_kind() {
        let name = |p: &str| OpenFile::parse(ID, p).unwrap().file_name();
        assert_eq!(name("out/Report 2026.pdf"), "Report 2026.pdf");
        assert_eq!(name("photo.JPEG"), "photo.jpg");
        assert_eq!(name("..hidden.png"), "hidden.png");
        assert_eq!(name(".png"), "file.png");
        assert_eq!(name("a*b|c\"<>.gif"), "a_b_c___.gif");
        assert_eq!(name(&format!("{}.png", "x".repeat(200))), format!("{}.png", "x".repeat(MAX_NAME_CHARS)));
    }

    #[test]
    fn the_bytes_must_be_what_the_name_says() {
        assert!(Kind::Png.matches(b"\x89PNG\r\n\x1a\nrest"));
        assert!(!Kind::Png.matches(b"<html><script>"));
        assert!(Kind::Jpeg.matches(b"\xff\xd8\xff\xe0"));
        assert!(Kind::Gif.matches(b"GIF89a..."));
        assert!(Kind::Webp.matches(b"RIFF\0\0\0\0WEBPVP8 "));
        assert!(!Kind::Webp.matches(b"RIFF\0\0\0\0WAVE"));
        assert!(Kind::Avif.matches(b"\0\0\0\x1cftypavif"));
        assert!(Kind::Pdf.matches(b"\n%PDF-1.7"));
        assert!(!Kind::Pdf.matches(b"<svg onload=alert(1)>"));
        assert!(!Kind::Pdf.matches(b""));
    }

    #[test]
    fn each_file_gets_a_private_folder_of_its_own() {
        let root = std::env::temp_dir().join(format!("cedar-open-test-{}", std::process::id()));
        let a = save(&root, "a.png", b"one").unwrap();
        let b = save(&root, "a.png", b"two").unwrap();
        assert_ne!(a.parent(), b.parent());
        assert_eq!(a.parent().unwrap().parent().unwrap(), root);
        assert_eq!(std::fs::read(&b).unwrap(), b"two");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&root), 0o700);
            assert_eq!(mode(a.parent().unwrap()), 0o700);
            assert_eq!(mode(&a), 0o600);
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
}
