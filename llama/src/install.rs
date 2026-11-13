//! Getting a build of llama.cpp and a model onto the disk: download (picking up where an earlier
//! try stopped), check against the pinned size and SHA-256, unpack. Nothing needs administrator
//! rights; everything goes into folders the caller names.

use crate::catalog::Model;
use crate::release::{Asset, Build, Manifest};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

/// Why an install stopped. The desktop app says these in the user's language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The server could not be reached or refused; the text says which.
    Download(String),
    /// Nothing arrived for a minute.
    Stalled,
    /// The connection ended before the file did. What arrived is kept for the next try.
    Cut,
    /// What arrived is not the file that was pinned.
    Changed(String),
    /// The caller cancelled. What arrived is kept for the next try.
    Cancelled,
    NoSpace {
        need: u64,
        free: u64,
    },
    Unpack(String),
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Download(e) => write!(f, "the download failed: {e}"),
            Self::Stalled => write!(f, "the download stalled"),
            Self::Cut => write!(f, "the download was cut off; try again to continue it"),
            Self::Changed(name) => write!(
                f,
                "{name} is not the file Cedar expects, so it was not used"
            ),
            Self::Cancelled => write!(f, "cancelled"),
            Self::NoSpace { need, free } => {
                write!(
                    f,
                    "not enough free disk space: {} GB needed, {} GB free",
                    gb(*need),
                    gb(*free)
                )
            }
            Self::Unpack(e) => write!(f, "unpacking failed: {e}"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

fn gb(bytes: u64) -> u64 {
    bytes.div_ceil(1_000_000_000)
}

/// A request's error with its causes, which is where reqwest says what went wrong (a refused
/// connection, a certificate that is not trusted).
fn causes(e: reqwest::Error) -> Error {
    let mut rest = String::new();
    let mut source = std::error::Error::source(&e);
    while let Some(cause) = source {
        rest.push_str(&format!(": {cause}"));
        source = cause.source();
    }
    Error::Download(format!("{}{rest}", e.without_url()))
}

fn io(what: &str, path: &Path, e: std::io::Error) -> Error {
    Error::Io(format!("{what} {}: {e}", path.display()))
}

/// What an install is doing, for a progress display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Bytes on the disk so far and in all, over every file of this step.
    Downloading {
        got: u64,
        total: u64,
    },
    /// Hashing what arrived, which takes a while for a model.
    Checking,
    Unpacking,
}

const STALL: Duration = Duration::from_secs(60);

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

async fn file_len(path: &Path) -> Option<u64> {
    tokio::fs::metadata(path)
        .await
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.len())
}

/// The bytes of a file that are on the disk already: all of it, or what an earlier try left.
async fn have(dest: &Path, size: u64) -> u64 {
    if file_len(dest).await == Some(size) {
        return size;
    }
    file_len(&part_path(dest))
        .await
        .filter(|n| *n <= size)
        .unwrap_or(0)
}

/// Downloads `url` to `dest`, which must end up `size` bytes with the hash `sha256`. A file of
/// that size already there is taken as done (it got its name only after its hash was checked).
/// A partial download left by an earlier try is continued where the server allows.
/// `on_progress` gets the bytes on the disk as they grow, then `None` while the hash is checked.
pub async fn fetch(
    url: &str,
    dest: &Path,
    size: u64,
    sha256: &str,
    cancel: &CancellationToken,
    on_progress: impl Fn(Option<u64>),
) -> Result<(), Error> {
    if file_len(dest).await == Some(size) {
        return Ok(());
    }
    if let Some(dir) = dest.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| io("creating", dir, e))?;
    }
    let part = part_path(dest);
    let mut got = have(dest, size).await;
    if got < size {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .build()
            .map_err(causes)?;
        let mut request = client.get(url);
        if got > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={got}-"));
        }
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            r = request.send() => r.map_err(causes)?,
        };
        match response.status() {
            reqwest::StatusCode::PARTIAL_CONTENT => {}
            // The whole file again: the server does not continue downloads.
            s if s.is_success() => got = 0,
            s => {
                return Err(Error::Download(format!(
                    "{} answered {s}",
                    response.url().host_str().unwrap_or(url)
                )))
            }
        }
        let mut options = tokio::fs::OpenOptions::new();
        if got > 0 {
            options.append(true);
        } else {
            options.write(true).create(true).truncate(true);
        }
        let mut file = options
            .open(&part)
            .await
            .map_err(|e| io("writing", &part, e))?;
        let mut stream = response.bytes_stream();
        let mut shown = got;
        on_progress(Some(got));
        let ended = loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => break Err(Error::Cancelled),
                next = tokio::time::timeout(STALL, stream.next()) => match next {
                    Err(_) => break Err(Error::Stalled),
                    Ok(None) => break Ok(()),
                    Ok(Some(Err(_))) => break Err(Error::Cut),
                    Ok(Some(Ok(chunk))) => chunk,
                },
            };
            if let Err(e) = file.write_all(&chunk).await {
                break Err(io("writing", &part, e));
            }
            got += chunk.len() as u64;
            if got - shown >= 1 << 20 {
                shown = got;
                on_progress(Some(got));
            }
        };
        // What arrived stays on the disk for the next try, whatever stopped this one.
        let flushed = file.flush().await;
        drop(file);
        ended?;
        flushed.map_err(|e| io("writing", &part, e))?;
        if got < size {
            return Err(Error::Cut);
        }
    }

    on_progress(None);
    let name = dest
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let hashed = {
        let part = part.clone();
        tokio::task::spawn_blocking(move || sha256_file(&part))
            .await
            .map_err(|e| Error::Io(e.to_string()))?
    };
    let right = got == size && hashed.as_ref().is_ok_and(|h| h == sha256);
    if !right {
        // Not something to continue from.
        let _ = tokio::fs::remove_file(&part).await;
        hashed.map_err(|e| io("reading", &part, e))?;
        return Err(Error::Changed(name));
    }
    tokio::fs::rename(&part, dest)
        .await
        .map_err(|e| io("saving", dest, e))
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(hex::encode(hasher.finalize()));
        }
        hasher.update(&buf[..n]);
    }
}

/// Unpacks a build's archive into `dir`: a `.zip` as it is, a `.tar.gz` without its top folder
/// (upstream's are `llama-<tag>/…`), so that several archives of a build land side by side.
/// Only files, folders and links to a file in the same folder are taken; upstream's archives hold
/// nothing else.
pub fn unpack(archive: &Path, dir: &Path) -> Result<(), Error> {
    let bad = |e: &dyn fmt::Display| Error::Unpack(format!("{}: {e}", archive.display()));
    std::fs::create_dir_all(dir).map_err(|e| io("creating", dir, e))?;
    let file = std::fs::File::open(archive).map_err(|e| io("opening", archive, e))?;
    if archive.extension().is_some_and(|e| e == "zip") {
        return zip::ZipArchive::new(file)
            .and_then(|mut z| z.extract(dir))
            .map_err(|e| bad(&e));
    }
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    for entry in tar.entries().map_err(|e| bad(&e))? {
        let mut entry = entry.map_err(|e| bad(&e))?;
        let path = entry.path().map_err(|e| bad(&e))?.into_owned();
        let mut inside = PathBuf::new();
        for part in path
            .components()
            .filter(|c| !matches!(c, Component::CurDir))
            .skip(1)
        {
            match part {
                Component::Normal(name) => inside.push(name),
                _ => {
                    return Err(bad(&format!(
                        "the path {} leaves the folder",
                        path.display()
                    )))
                }
            }
        }
        if inside.as_os_str().is_empty() {
            continue;
        }
        let kind = entry.header().entry_type();
        let link_in_folder = kind.is_symlink()
            && entry.link_name().ok().flatten().is_some_and(|target| {
                let mut parts = target.components();
                matches!(
                    (parts.next(), parts.next()),
                    (Some(Component::Normal(_)), None)
                )
            });
        if !(kind.is_file() || kind.is_dir() || link_in_folder) {
            return Err(bad(&format!(
                "{} is not a file, a folder or a link beside it",
                path.display()
            )));
        }
        let dest = dir.join(&inside);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io("creating", parent, e))?;
        }
        entry.unpack(&dest).map_err(|e| bad(&e))?;
    }
    Ok(())
}

/// The folder a build of a release is unpacked into, under the folder for all builds.
pub fn build_dir(root: &Path, tag: &str, build: Build) -> PathBuf {
    root.join(format!("{tag}-{}", build.id()))
}

/// `llama-server` of a build, if that build is installed.
pub fn installed_server(root: &Path, tag: &str, build: Build) -> Option<PathBuf> {
    let exe = build_dir(root, tag, build).join(build.server_exe());
    exe.is_file().then_some(exe)
}

fn free_space(dir: &Path) -> Option<u64> {
    let existing = dir.ancestors().find(|p| p.exists())?;
    fs4::available_space(existing).ok()
}

fn need_space(dir: &Path, need: u64) -> Result<(), Error> {
    match free_space(dir) {
        Some(free) if free < need => Err(Error::NoSpace { need, free }),
        _ => Ok(()),
    }
}

/// Installs `build` of the pinned release under `root` and returns its `llama-server`. Already
/// there: nothing is downloaded. The folder gets its name only once everything is unpacked, so a
/// try that stopped halfway is never taken for an install; builds of other releases and other
/// GPUs are removed at the end.
pub async fn install_build(
    manifest: &Manifest,
    build: Build,
    root: &Path,
    cancel: &CancellationToken,
    on_event: impl Fn(Event),
) -> Result<PathBuf, Error> {
    if let Some(exe) = installed_server(root, &manifest.tag, build) {
        return Ok(exe);
    }
    let assets: Vec<Asset> = manifest
        .assets(build)
        .ok_or_else(|| Error::Unpack(format!("release {} has no {}", manifest.tag, build.id())))?;
    let downloads = root.join("downloads");
    let total: u64 = assets.iter().map(|a| a.size).sum();
    let mut done = 0;
    for asset in &assets {
        done += have(&downloads.join(&asset.name), asset.size).await;
    }
    // The archives, and about twice their size unpacked.
    need_space(root, (total - done) + 2 * total)?;

    let mut before = 0;
    for asset in &assets {
        let dest = downloads.join(&asset.name);
        fetch(
            &asset.url,
            &dest,
            asset.size,
            &asset.sha256,
            cancel,
            |got| match got {
                Some(got) => on_event(Event::Downloading {
                    got: before + got,
                    total,
                }),
                None => on_event(Event::Checking),
            },
        )
        .await?;
        before += asset.size;
    }

    on_event(Event::Unpacking);
    let dir = build_dir(root, &manifest.tag, build);
    let staging = dir.with_extension("tmp");
    let archives: Vec<PathBuf> = assets.iter().map(|a| downloads.join(&a.name)).collect();
    let unpacked = {
        let staging = staging.clone();
        tokio::task::spawn_blocking(move || {
            let _ = std::fs::remove_dir_all(&staging);
            archives
                .iter()
                .try_for_each(|archive| unpack(archive, &staging))
        })
        .await
        .map_err(|e| Error::Io(e.to_string()))?
    };
    if let Err(e) = unpacked {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(e);
    }
    if !staging.join(build.server_exe()).is_file() {
        let _ = tokio::fs::remove_dir_all(&staging).await;
        return Err(Error::Unpack(format!(
            "{} has no {}",
            assets[0].name,
            build.server_exe()
        )));
    }
    let _ = tokio::fs::remove_dir_all(&dir).await;
    tokio::fs::rename(&staging, &dir)
        .await
        .map_err(|e| io("saving", &dir, e))?;

    // The archives, and whatever an earlier version of Cedar installed.
    if let Ok(mut entries) = tokio::fs::read_dir(root).await {
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry.path() != dir {
                let _ = tokio::fs::remove_dir_all(entry.path()).await;
            }
        }
    }
    Ok(dir.join(build.server_exe()))
}

/// Where a model's file is kept under the folder for models.
pub fn model_path(models: &Path, model: &Model) -> PathBuf {
    models.join(model.file)
}

/// A model's file, if it is fully downloaded.
pub fn installed_model(models: &Path, model: &Model) -> Option<PathBuf> {
    let path = model_path(models, model);
    let whole = std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == model.size);
    whole.then_some(path)
}

/// Downloads `model` into `models` and returns its file. Other models are left alone: they are
/// large, and the user may switch back.
pub async fn install_model(
    model: &Model,
    models: &Path,
    cancel: &CancellationToken,
    on_event: impl Fn(Event),
) -> Result<PathBuf, Error> {
    let dest = model_path(models, model);
    let on_disk = have(&dest, model.size).await;
    if on_disk < model.size {
        need_space(models, model.size - on_disk)?;
    }
    fetch(
        &model.url(),
        &dest,
        model.size,
        model.sha256,
        cancel,
        |got| match got {
            Some(got) => on_event(Event::Downloading {
                got,
                total: model.size,
            }),
            None => on_event(Event::Checking),
        },
    )
    .await?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("cedar-llama-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sha(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    /// A server for one file that honours `Range` (unless `ranges` is off), sends at most `limit`
    /// bytes per request before it drops the connection, and records each request's `Range`.
    async fn serve(
        body: Vec<u8>,
        ranges: bool,
        limit: usize,
    ) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/file.bin", listener.local_addr().unwrap());
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = socket.read(&mut buf).await.unwrap();
                    if n == 0 {
                        break;
                    }
                    request.extend_from_slice(&buf[..n]);
                }
                let request = String::from_utf8_lossy(&request).to_lowercase();
                let range = request
                    .lines()
                    .find_map(|l| l.strip_prefix("range: bytes="))
                    .map(|r| r.trim().to_string())
                    .unwrap_or_default();
                log.lock().unwrap().push(range.clone());
                let from: usize = range.trim_end_matches('-').parse().unwrap_or(0);
                let (status, rest) = if ranges && from > 0 {
                    ("206 Partial Content", &body[from..])
                } else {
                    ("200 OK", &body[..])
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    rest.len()
                );
                let _ = socket.write_all(head.as_bytes()).await;
                let _ = socket.write_all(&rest[..rest.len().min(limit)]).await;
                let _ = socket.shutdown().await;
            }
        });
        (url, seen)
    }

    #[tokio::test]
    async fn a_download_is_checked_and_continued() {
        let dir = temp("fetch");
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let (size, hash) = (body.len() as u64, sha(&body));
        let dest = dir.join("file.bin");
        let cancel = CancellationToken::new();

        // The connection drops after 100,000 bytes: they stay, and the next try asks for the rest.
        let (url, seen) = serve(body.clone(), true, 100_000).await;
        let first = fetch(&url, &dest, size, &hash, &cancel, |_| {}).await;
        assert_eq!(first, Err(Error::Cut));
        assert_eq!(std::fs::metadata(part_path(&dest)).unwrap().len(), 100_000);
        assert!(!dest.exists());
        assert_eq!(
            fetch(&url, &dest, size, &hash, &cancel, |_| {}).await,
            Err(Error::Cut)
        );
        let steps = AtomicUsize::new(0);
        let third = fetch(&url, &dest, size, &hash, &cancel, |got| {
            steps.fetch_add(1, Ordering::Relaxed);
            assert!(got.is_none_or(|g| (200_000..=300_000).contains(&g)));
        })
        .await;
        assert_eq!(third, Ok(()));
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(!part_path(&dest).exists());
        assert_eq!(*seen.lock().unwrap(), ["", "100000-", "200000-"]);
        assert!(steps.load(Ordering::Relaxed) >= 2);

        // Once it is there, nothing is asked of the server.
        assert_eq!(
            fetch(
                "http://127.0.0.1:1/gone",
                &dest,
                size,
                &hash,
                &cancel,
                |_| {}
            )
            .await,
            Ok(())
        );
    }

    #[tokio::test]
    async fn a_server_without_ranges_starts_over() {
        let dir = temp("noranges");
        let body = vec![7u8; 50_000];
        let dest = dir.join("file.bin");
        std::fs::write(part_path(&dest), vec![9u8; 20_000]).unwrap();
        let (url, _) = serve(body.clone(), false, usize::MAX).await;
        let cancel = CancellationToken::new();
        assert_eq!(
            fetch(&url, &dest, 50_000, &sha(&body), &cancel, |_| {}).await,
            Ok(())
        );
        assert_eq!(std::fs::read(&dest).unwrap(), body);
    }

    #[tokio::test]
    async fn a_changed_file_is_refused() {
        let dir = temp("changed");
        let body = vec![1u8; 10_000];
        let dest = dir.join("file.bin");
        let (url, _) = serve(body.clone(), true, usize::MAX).await;
        let cancel = CancellationToken::new();
        let wrong = sha(b"something else");
        let got = fetch(&url, &dest, 10_000, &wrong, &cancel, |_| {}).await;
        assert_eq!(got, Err(Error::Changed("file.bin".into())));
        // Neither kept as the file nor as something to continue.
        assert!(!dest.exists() && !part_path(&dest).exists());
        // A longer file than pinned is refused the same way.
        let got = fetch(&url, &dest, 9_000, &sha(&body[..9_000]), &cancel, |_| {}).await;
        assert_eq!(got, Err(Error::Changed("file.bin".into())));
        assert!(!dest.exists() && !part_path(&dest).exists());
    }

    #[tokio::test]
    async fn cancelling_keeps_what_arrived() {
        let dir = temp("cancel");
        let dest = dir.join("file.bin");
        let cancel = CancellationToken::new();
        cancel.cancel();
        let (url, _) = serve(vec![0u8; 1000], true, usize::MAX).await;
        assert_eq!(
            fetch(&url, &dest, 1000, &sha(&[0u8; 1000]), &cancel, |_| {}).await,
            Err(Error::Cancelled)
        );
        assert!(!dest.exists());
    }

    #[tokio::test]
    async fn a_refusing_server_is_named() {
        let dir = temp("refused");
        let cancel = CancellationToken::new();
        let got = fetch(
            "http://127.0.0.1:1/file.bin",
            &dir.join("f"),
            10,
            &sha(b"x"),
            &cancel,
            |_| {},
        )
        .await;
        assert!(matches!(got, Err(Error::Download(_))), "{got:?}");
    }

    fn tar_gz(
        path: &Path,
        build: impl FnOnce(&mut tar::Builder<flate2::write::GzEncoder<std::fs::File>>),
    ) {
        let gz = flate2::write::GzEncoder::new(
            std::fs::File::create(path).unwrap(),
            flate2::Compression::fast(),
        );
        let mut tar = tar::Builder::new(gz);
        build(&mut tar);
        tar.into_inner().unwrap().finish().unwrap();
    }

    fn add_file(tar: &mut tar::Builder<impl std::io::Write>, path: &str, body: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_size(body.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, path, body).unwrap();
    }

    fn add_link(tar: &mut tar::Builder<impl std::io::Write>, path: &str, target: &str) {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        tar.append_link(&mut header, path, target).unwrap();
    }

    #[test]
    fn a_tarball_loses_its_top_folder() {
        let dir = temp("untar");
        let archive = dir.join("llama-b1-bin-ubuntu-vulkan-x64.tar.gz");
        tar_gz(&archive, |tar| {
            tar.append_dir("llama-b1", &dir).unwrap();
            add_file(tar, "llama-b1/llama-server", b"#!/bin/sh\n");
            add_file(tar, "llama-b1/libllama.so.1.0.0", b"lib");
            #[cfg(unix)]
            add_link(tar, "llama-b1/libllama.so", "libllama.so.1.0.0");
        });
        let out = dir.join("out");
        unpack(&archive, &out).unwrap();
        assert_eq!(
            std::fs::read(out.join("llama-server")).unwrap(),
            b"#!/bin/sh\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::read(out.join("libllama.so")).unwrap(), b"lib");
            assert!(out
                .join("libllama.so")
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink());
            assert_ne!(
                out.join("llama-server")
                    .metadata()
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o111,
                0
            );
        }
        assert!(!out.join("llama-b1").exists());
    }

    #[test]
    #[cfg(unix)]
    fn a_tarball_cannot_reach_outside() {
        let dir = temp("escape");
        for (name, target) in [
            ("up", "../../outside"),
            ("absolute", "/etc/passwd"),
            ("deep", "sub/file"),
        ] {
            let archive = dir.join(format!("{name}.tar.gz"));
            tar_gz(&archive, |tar| add_link(tar, "llama-b1/link", target));
            let got = unpack(&archive, &dir.join(name));
            assert!(matches!(got, Err(Error::Unpack(_))), "{name}: {got:?}");
            assert!(dir.join(name).join("link").symlink_metadata().is_err());
        }
        // A path with `..` in it never gets written either: tar's own builder refuses to make
        // one, so the header is filled in by hand.
        let archive = dir.join("dots.tar.gz");
        tar_gz(&archive, |tar| {
            let mut header = tar::Header::new_gnu();
            let name = b"llama-b1/../../evil";
            header.as_old_mut().name[..name.len()].copy_from_slice(name);
            header.set_size(1);
            header.set_cksum();
            tar.append(&header, &b"x"[..]).unwrap();
        });
        assert!(matches!(
            unpack(&archive, &dir.join("dots")),
            Err(Error::Unpack(_))
        ));
        assert!(!dir.join("evil").exists());
    }

    #[test]
    fn a_zip_is_unpacked_as_it_is() {
        use std::io::Write;
        let dir = temp("unzip");
        let archive = dir.join("llama-b1-bin-win-vulkan-x64.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("llama-server.exe", options).unwrap();
        zip.write_all(b"MZ").unwrap();
        zip.start_file("ggml-vulkan.dll", options).unwrap();
        zip.write_all(b"dll").unwrap();
        zip.finish().unwrap();
        let out = dir.join("out");
        unpack(&archive, &out).unwrap();
        assert_eq!(std::fs::read(out.join("llama-server.exe")).unwrap(), b"MZ");
        assert_eq!(std::fs::read(out.join("ggml-vulkan.dll")).unwrap(), b"dll");
    }

    #[test]
    fn errors_read_as_sentences() {
        let e = Error::NoSpace {
            need: 20_600_000_000,
            free: 4_100_000_000,
        };
        assert_eq!(
            e.to_string(),
            "not enough free disk space: 21 GB needed, 5 GB free"
        );
    }
}
