//! Pulling an image through the Docker Engine API, for the progress in bytes that `docker pull`
//! only prints to a terminal, and following a pull's layers to show how far it is.
//!
//! The API is reached on the socket of the CLI's current context (`docker context inspect`), so
//! it is the same daemon the CLI talks to: Docker Desktop, OrbStack, Colima, and Rancher Desktop
//! on a Unix socket, Docker Desktop and Rancher Desktop on Windows on a named pipe. Anything else
//! (a TCP or SSH context) is left to `docker pull`, as is a pull the API could not finish (see
//! `DockerCli::pull`).

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// How far a pull is. Shown on the setup page as a bar and a line of text.
#[derive(Serialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct PullProgress {
    /// The layers to download; 0 until the daemon has listed them. Layers already on disk are
    /// not counted.
    pub layers: usize,
    /// Of those, the ones downloaded.
    pub downloaded: usize,
    /// Of those, the ones unpacked too.
    pub unpacked: usize,
    /// The bytes downloaded so far, of the layers whose size the daemon reported.
    pub bytes: u64,
    /// The size of all the layers, once it is known for every layer still downloading.
    pub total_bytes: Option<u64>,
    /// How far the whole pull is, from 0 to 1; never goes back. None until layers are listed.
    pub fraction: Option<f64>,
}

/// One layer of a pull.
#[derive(Default)]
struct Layer {
    /// Its size, compressed, once the daemon reports it. Small layers often finish without one.
    total: Option<u64>,
    current: u64,
    downloaded: bool,
    unpacked: bool,
    /// On disk already; nothing to do.
    exists: bool,
}

/// Downloading is three quarters of a layer's work and unpacking the rest. Unpacking reports no
/// bytes (the containerd image store reports seconds), so a layer is unpacked or not.
const DOWNLOAD_SHARE: f64 = 0.75;

/// Follows a pull's layers through the daemon's messages, from the API or from `docker pull`'s
/// lines. A layer is listed first (`Pulling fs layer`, or `Already exists`); other messages only
/// update listed layers, since the containerd image store also reports the image's config and
/// manifest as `Download complete`.
#[derive(Default)]
pub struct PullLayers {
    layers: HashMap<String, Layer>,
    best: f64,
}

impl PullLayers {
    /// A message about the layer `id`, with the bytes downloaded and its size when given.
    pub fn read(&mut self, id: &str, status: &str, bytes: Option<(u64, u64)>) {
        match status {
            "Pulling fs layer" | "Waiting" => {
                self.layers.entry(id.to_string()).or_default();
                return;
            }
            "Already exists" => {
                self.layers.entry(id.to_string()).or_default().exists = true;
                return;
            }
            _ => {}
        }
        let Some(layer) = self.layers.get_mut(id) else {
            return;
        };
        match status {
            "Downloading" => {
                if let Some((current, total)) = bytes {
                    if total > 0 {
                        layer.total = Some(total);
                        layer.current = current.min(total);
                    }
                }
            }
            "Verifying Checksum" | "Download complete" | "Extracting" => layer.downloaded = true,
            "Pull complete" => {
                layer.downloaded = true;
                layer.unpacked = true;
            }
            _ => {}
        }
    }

    /// A line of `docker pull` without a terminal: `<id>: <status>`, with no byte counts.
    pub fn read_line(&mut self, line: &str) {
        if let Some((id, status)) = line.split_once(": ") {
            self.read(id, status.trim(), None);
        }
    }

    pub fn progress(&mut self) -> PullProgress {
        let fetch: Vec<&Layer> = self.layers.values().filter(|l| !l.exists).collect();
        let mut p = PullProgress {
            layers: fetch.len(),
            downloaded: fetch.iter().filter(|l| l.downloaded).count(),
            unpacked: fetch.iter().filter(|l| l.unpacked).count(),
            ..Default::default()
        };
        if fetch.is_empty() {
            return p;
        }
        let sizes: Vec<u64> = fetch.iter().filter_map(|l| l.total).collect();
        p.bytes = fetch
            .iter()
            .filter_map(|l| l.total.map(|t| if l.downloaded { t } else { l.current }))
            .sum();
        if !sizes.is_empty() && fetch.iter().all(|l| l.downloaded || l.total.is_some()) {
            p.total_bytes = Some(sizes.iter().sum());
        }
        // Each layer weighs its size. One still waiting weighs the average size so far, and a
        // small one that finished without reporting a size weighs nothing. Before any size is
        // known, the layers weigh the same.
        let average = (!sizes.is_empty()).then(|| sizes.iter().sum::<u64>() as f64 / sizes.len() as f64);
        let (mut done, mut all) = (0.0, 0.0);
        for l in &fetch {
            let weight = match (l.total, average) {
                (Some(t), _) => t as f64,
                (None, None) => 1.0,
                (None, Some(_)) if l.downloaded => 0.0,
                (None, Some(a)) => a,
            };
            let downloaded = match l.total {
                _ if l.downloaded => 1.0,
                Some(t) => l.current as f64 / t as f64,
                None => 0.0,
            };
            let unpacked = if l.unpacked { 1.0 } else { 0.0 };
            done += weight * (DOWNLOAD_SHARE * downloaded + (1.0 - DOWNLOAD_SHARE) * unpacked);
            all += weight;
        }
        let fraction = if all > 0.0 { done / all } else { 0.0 };
        // A layer whose size turns out larger than the average it stood in for would move the
        // bar back; it waits instead.
        self.best = self.best.max(fraction.clamp(0.0, 1.0));
        p.fraction = Some(self.best);
        p
    }
}

/// Where the daemon listens, from a context's `Host`.
#[derive(Debug, PartialEq)]
pub enum Endpoint {
    Unix(String),
    /// A Windows named pipe, as `\\.\pipe\docker_engine`.
    Pipe(String),
}

impl Endpoint {
    /// `unix:///var/run/docker.sock` or `npipe:////./pipe/docker_engine`; None for others.
    pub fn parse(host: &str) -> Option<Self> {
        let host = host.trim();
        if let Some(path) = host.strip_prefix("unix://") {
            return (!path.is_empty()).then(|| Endpoint::Unix(path.to_string()));
        }
        if let Some(path) = host.strip_prefix("npipe://") {
            return (!path.is_empty()).then(|| Endpoint::Pipe(path.replace('/', "\\")));
        }
        None
    }
}

trait Conn: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Conn for T {}

async fn connect(endpoint: &Endpoint) -> Result<Box<dyn Conn>> {
    match endpoint {
        #[cfg(unix)]
        Endpoint::Unix(path) => Ok(Box::new(
            tokio::net::UnixStream::connect(path)
                .await
                .with_context(|| format!("connecting to {path}"))?,
        )),
        #[cfg(windows)]
        Endpoint::Pipe(name) => {
            use tokio::net::windows::named_pipe::ClientOptions;
            // ERROR_PIPE_BUSY: every instance is taken for the moment.
            const PIPE_BUSY: i32 = 231;
            let mut tries = 0;
            loop {
                match ClientOptions::new().open(name) {
                    Ok(pipe) => return Ok(Box::new(pipe)),
                    Err(e) if e.raw_os_error() == Some(PIPE_BUSY) && tries < 20 => {
                        tries += 1;
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                    Err(e) => return Err(e).with_context(|| format!("connecting to {name}")),
                }
            }
        }
        #[allow(unreachable_patterns)]
        other => bail!("{other:?} is not reachable on this system"),
    }
}

/// An image reference as the API's `fromImage` and `tag`: `ghcr.io/a/b:1.2` is `ghcr.io/a/b`
/// and `1.2`, `a/b@sha256:…` is `a/b` and the digest, and no tag is `latest`.
fn split_reference(image: &str) -> (&str, &str) {
    if let Some((name, digest)) = image.split_once('@') {
        return (name, digest);
    }
    let after_slash = image.rfind('/').map_or(0, |i| i + 1);
    match image[after_slash..].rfind(':') {
        Some(i) => (&image[..after_slash + i], &image[after_slash + i + 1..]),
        None => (image, "latest"),
    }
}

fn query_escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// A message of the pull's stream.
#[derive(Deserialize)]
struct Message {
    status: Option<String>,
    id: Option<String>,
    #[serde(rename = "progressDetail")]
    detail: Option<Detail>,
    error: Option<String>,
    #[serde(rename = "errorDetail")]
    error_detail: Option<ErrorDetail>,
}

#[derive(Deserialize)]
struct Detail {
    current: Option<u64>,
    total: Option<u64>,
    /// Set (to `s`) when `current` counts something other than bytes.
    units: Option<String>,
}

#[derive(Deserialize)]
struct ErrorDetail {
    message: Option<String>,
}

/// The body of an HTTP/1.1 response, chunked or not.
struct Body<R> {
    r: R,
    chunked: bool,
    left_in_chunk: usize,
    done: bool,
}

impl<R: AsyncBufReadExt + Unpin> Body<R> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        if self.done {
            return Ok(0);
        }
        if !self.chunked {
            let n = self.r.read(buf).await?;
            self.done = n == 0;
            return Ok(n);
        }
        if self.left_in_chunk == 0 {
            let mut line = String::new();
            if self.r.read_line(&mut line).await? == 0 {
                bail!("the daemon closed the connection mid-response");
            }
            let size = line.trim().split(';').next().unwrap_or("");
            self.left_in_chunk =
                usize::from_str_radix(size, 16).with_context(|| format!("bad chunk size {size:?}"))?;
            if self.left_in_chunk == 0 {
                self.done = true;
                return Ok(0);
            }
        }
        let want = buf.len().min(self.left_in_chunk);
        let n = self.r.read(&mut buf[..want]).await?;
        if n == 0 {
            bail!("the daemon closed the connection mid-response");
        }
        self.left_in_chunk -= n;
        if self.left_in_chunk == 0 {
            let mut crlf = [0u8; 2];
            self.r.read_exact(&mut crlf).await?;
        }
        Ok(n)
    }
}

/// Pulls `image` through the Engine API at `endpoint`, reporting each message as a line (like
/// `docker pull`'s) and how far the pull is. Sends no registry credentials, so it suits public
/// images; an error is the daemon's own message.
pub async fn pull(
    endpoint: &Endpoint,
    image: &str,
    mut on_progress: impl FnMut(String, PullProgress),
) -> Result<()> {
    let (name, tag) = split_reference(image);
    let mut conn = connect(endpoint).await?;
    let request = format!(
        "POST /images/create?fromImage={}&tag={} HTTP/1.1\r\nHost: docker\r\nUser-Agent: cedar-app\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        query_escape(name),
        query_escape(tag),
    );
    conn.write_all(request.as_bytes()).await?;
    conn.flush().await?;

    let mut r = BufReader::new(conn);
    let mut status_line = String::new();
    r.read_line(&mut status_line).await?;
    let code: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| anyhow!("not an HTTP response: {:?}", status_line.trim()))?;
    let mut chunked = false;
    loop {
        let mut header = String::new();
        if r.read_line(&mut header).await? == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((key, value)) = header.split_once(':') {
            if key.trim().eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
            {
                chunked = true;
            }
        }
    }
    let mut body = Body { r, chunked, left_in_chunk: 0, done: false };

    if code != 200 {
        let mut text = Vec::new();
        let mut buf = [0u8; 4096];
        while text.len() < 64 * 1024 {
            let n = body.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            text.extend_from_slice(&buf[..n]);
        }
        let text = String::from_utf8_lossy(&text);
        let message = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_string))
            .unwrap_or_else(|| text.trim().to_string());
        bail!("{message} (HTTP {code})");
    }

    let mut layers = PullLayers::default();
    let mut pending = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = body.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        pending.extend_from_slice(&buf[..n]);
        while let Some(end) = pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            handle(&line, &mut layers, &mut on_progress)?;
        }
    }
    handle(&pending, &mut layers, &mut on_progress)
}

/// One message of the stream; the error the daemon reports in it, if any.
fn handle(
    line: &[u8],
    layers: &mut PullLayers,
    on_progress: &mut impl FnMut(String, PullProgress),
) -> Result<()> {
    let line = String::from_utf8_lossy(line);
    let line = line.trim();
    if line.is_empty() {
        return Ok(());
    }
    let Ok(m) = serde_json::from_str::<Message>(line) else {
        return Ok(());
    };
    if let Some(error) = m.error_detail.and_then(|d| d.message).or(m.error) {
        bail!("{error}");
    }
    let status = m.status.unwrap_or_default();
    let bytes = m.detail.and_then(|d| match (d.current, d.total, d.units) {
        (Some(current), Some(total), None) => Some((current, total)),
        _ => None,
    });
    let text = match &m.id {
        Some(id) => {
            layers.read(id, &status, bytes);
            format!("{id}: {status}")
        }
        None => status,
    };
    on_progress(text, layers.progress());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_lines_count_layers_not_config_blobs() {
        // `docker pull` without a terminal, containerd image store.
        let mut layers = PullLayers::default();
        for line in [
            "3.11-slim: Pulling from library/python",
            "37fb86c2a767: Pulling fs layer",
            "bbe7053e6def: Pulling fs layer",
            "53daecf7856e: Pulling fs layer",
            "7efbce397249: Pulling fs layer",
            "37fb86c2a767: Already exists",
            "50e2aab0fb81: Download complete",
            "e0063bf9ebd8: Download complete",
            "bbe7053e6def: Download complete",
            "53daecf7856e: Download complete",
            "53daecf7856e: Pull complete",
        ] {
            layers.read_line(line);
        }
        let p = layers.progress();
        assert_eq!((p.layers, p.downloaded, p.unpacked), (3, 2, 1));
        assert_eq!(p.total_bytes, None);
        // Equal weights: (2 * 0.75 + 1 * 0.25) / 3.
        assert!((p.fraction.unwrap() - 1.75 / 3.0).abs() < 1e-9);
        for line in ["7efbce397249: Download complete", "bbe7053e6def: Pull complete", "7efbce397249: Pull complete"] {
            layers.read_line(line);
        }
        assert_eq!(layers.progress().fraction, Some(1.0));
    }

    #[test]
    fn bytes_weigh_layers_by_size_and_never_go_back() {
        let mut layers = PullLayers::default();
        assert_eq!(layers.progress().fraction, None);
        for id in ["small", "big", "later"] {
            layers.read(id, "Pulling fs layer", None);
        }
        layers.read("small", "Download complete", None);
        layers.read("big", "Downloading", Some((25, 100)));
        let p = layers.progress();
        assert_eq!((p.bytes, p.total_bytes), (25, None));
        // small weighs nothing, big 100, later the average (100): 0.75 * 25 / 200.
        assert!((p.fraction.unwrap() - 0.75 * 25.0 / 200.0).abs() < 1e-9);
        // `later` turns out ten times the average: the bar holds rather than going back.
        layers.read("later", "Downloading", Some((0, 1000)));
        let held = layers.progress();
        assert_eq!(held.fraction, p.fraction);
        assert_eq!(held.total_bytes, Some(1100));
        // Extracting reports seconds, not bytes, and does not count as bytes.
        layers.read("big", "Extracting", None);
        layers.read("later", "Downloading", Some((1000, 1000)));
        let p = layers.progress();
        assert_eq!(p.bytes, 1100);
        assert!((p.fraction.unwrap() - 0.75).abs() < 1e-9);
        for id in ["small", "big", "later"] {
            layers.read(id, "Pull complete", None);
        }
        assert_eq!(layers.progress().fraction, Some(1.0));
    }

    #[test]
    fn stream_messages() {
        let mut layers = PullLayers::default();
        let mut last = None;
        let mut note = |text: String, p: PullProgress| last = Some((text, p));
        for line in [
            r#"{"status":"Pulling from library/python","id":"3.12-slim"}"#,
            r#"{"status":"Pulling fs layer","progressDetail":{},"id":"2dc99d2d0a39"}"#,
            r#"{"status":"Download complete","progressDetail":{"hidecounts":true},"id":"e0a5f9afcd02"}"#,
            r#"{"status":"Downloading","progressDetail":{"current":3145728,"total":12122550},"id":"2dc99d2d0a39"}"#,
            r#"{"status":"Extracting","progressDetail":{"current":1,"units":"s"},"id":"2dc99d2d0a39"}"#,
        ] {
            handle(line.as_bytes(), &mut layers, &mut note).unwrap();
        }
        let err = handle(
            br#"{"error":"x","errorDetail":{"message":"denied: not allowed"}}"#,
            &mut layers,
            &mut note,
        );
        assert_eq!(err.unwrap_err().to_string(), "denied: not allowed");
        let (text, p) = last.unwrap();
        assert_eq!(text, "2dc99d2d0a39: Extracting");
        assert_eq!((p.layers, p.downloaded, p.bytes), (1, 1, 12122550));
    }

    #[test]
    fn references_and_endpoints() {
        assert_eq!(split_reference("ghcr.io/a/cedar-sandbox:0.6.4"), ("ghcr.io/a/cedar-sandbox", "0.6.4"));
        assert_eq!(split_reference("localhost:5000/img"), ("localhost:5000/img", "latest"));
        assert_eq!(split_reference("a/b@sha256:ab"), ("a/b", "sha256:ab"));
        assert_eq!(query_escape("sha256:ab"), "sha256%3Aab");
        assert_eq!(Endpoint::parse("unix:///var/run/docker.sock"), Some(Endpoint::Unix("/var/run/docker.sock".into())));
        assert_eq!(
            Endpoint::parse("npipe:////./pipe/dockerDesktopLinuxEngine"),
            Some(Endpoint::Pipe(r"\\.\pipe\dockerDesktopLinuxEngine".into()))
        );
        assert_eq!(Endpoint::parse("tcp://10.0.0.2:2376"), None);
        assert_eq!(Endpoint::parse("ssh://me@box"), None);
    }
}
