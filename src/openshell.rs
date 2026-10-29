//! Talking to OpenShell through its `openshell` command, the way cedar used to talk to Docker
//! through `docker`.
//!
//! The command reads which gateway to use from its own registration (`openshell gateway add`),
//! or from `OPENSHELL_GATEWAY` / `OPENSHELL_GATEWAY_ENDPOINT` in cedar's environment, which it
//! inherits; `OPENSHELL_WORKSPACE` picks the OpenShell workspace the same way. Using the command
//! rather than the gateway's gRPC API keeps cedar on the mTLS bundle the CLI already manages.

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// How long an ordinary `openshell` call (get, stop, delete, policy set, logs) may take.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// How long creating a sandbox may take: the first one pulls OpenShell's runtime images.
pub const CREATE_TIMEOUT: Duration = Duration::from_secs(300);

/// The `openshell` command.
#[derive(Debug, Clone)]
pub struct OpenShell {
    pub bin: String,
    /// The OpenShell workspace every call works in, named on each call so the command and the
    /// labels cedar puts on volumes always agree.
    pub workspace: String,
}

impl OpenShell {
    pub fn new(bin: impl Into<String>, workspace: impl Into<String>) -> Self {
        Self {
            bin: bin.into(),
            workspace: workspace.into(),
        }
    }

    /// A command with cedar's fixed preferences: its workspace, and no colours (its output is
    /// parsed).
    pub fn command(&self) -> Command {
        let mut cmd = Command::new(&self.bin);
        cmd.args(["--workspace", &self.workspace])
            .env("OPENSHELL_COLOR", "never")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        cmd
    }

    /// Run `openshell args...` and return its output, whatever the exit status.
    pub async fn output(&self, args: &[String], timeout: Duration) -> Result<std::process::Output> {
        let child =
            self.command().args(args).spawn().with_context(|| {
                format!("failed to run `{}` (is OpenShell installed?)", self.bin)
            })?;
        tokio::time::timeout(timeout, child.wait_with_output())
            .await
            .map_err(|_| anyhow!("`openshell {}` timed out", describe(args)))?
            .map_err(Into::into)
    }

    /// Like [`Self::output`], but a non-zero exit is an error carrying what it printed.
    pub async fn checked(&self, args: &[String], timeout: Duration) -> Result<String> {
        let out = self.output(args, timeout).await?;
        if !out.status.success() {
            bail!(
                "`openshell {}` failed ({}): {}",
                describe(args),
                out.status,
                error_text(&out)
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Run a command that prints JSON (`-o json`) and parse it.
    pub async fn json(&self, args: &[String], timeout: Duration) -> Result<Value> {
        let out = self.checked(args, timeout).await?;
        serde_json::from_str(&out)
            .with_context(|| format!("`openshell {}` did not print JSON", describe(args)))
    }

    /// The sandbox called `name`, as `openshell sandbox get -o json` describes it; `None` if
    /// there is no such sandbox.
    pub async fn sandbox(&self, name: &str) -> Result<Option<Value>> {
        let args = strings(&["sandbox", "get", name, "-o", "json"]);
        let out = self.output(&args, CALL_TIMEOUT).await?;
        if out.status.success() {
            return serde_json::from_slice(&out.stdout)
                .map(Some)
                .context("`openshell sandbox get` did not print JSON");
        }
        let text = error_text(&out);
        if is_not_found(&text) {
            return Ok(None);
        }
        bail!(
            "`openshell sandbox get {name}` failed ({}): {text}",
            out.status
        )
    }

    /// The names of the sandboxes carrying `label` (`key=value`), across pages.
    pub async fn names_with_label(&self, label: &str) -> Result<Vec<String>> {
        let mut names = Vec::new();
        let mut token = String::new();
        loop {
            let mut args = strings(&["sandbox", "list", "--selector", label, "-o", "json"]);
            if !token.is_empty() {
                args.extend(["--page-token".into(), token.clone()]);
            }
            let page = self.json(&args, CALL_TIMEOUT).await?;
            names.extend(
                page["sandboxes"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|s| s["name"].as_str().map(String::from)),
            );
            match page["next_page_token"].as_str() {
                Some(t) if !t.is_empty() => token = t.to_string(),
                _ => return Ok(names),
            }
        }
    }

    /// Delete sandboxes. One that is already gone is not an error.
    pub async fn delete(&self, names: &[String]) -> Result<()> {
        if names.is_empty() {
            return Ok(());
        }
        let mut args = strings(&["sandbox", "delete"]);
        args.extend(names.iter().cloned());
        let out = self.output(&args, CALL_TIMEOUT).await?;
        if out.status.success() || is_not_found(&error_text(&out)) {
            return Ok(());
        }
        bail!(
            "`openshell sandbox delete` failed ({}): {}",
            out.status,
            error_text(&out)
        )
    }

    /// Replace a running sandbox's policy with `policy` (a JSON document), and wait until the
    /// sandbox has loaded it. Only network rules change on a running sandbox.
    pub async fn set_policy(&self, name: &str, policy: &Value) -> Result<()> {
        let file = PolicyFile::write(policy)?;
        let args = vec![
            "policy".into(),
            "set".into(),
            name.into(),
            "--policy".into(),
            file.path_string(),
            "--wait".into(),
        ];
        self.checked(&args, CALL_TIMEOUT).await?;
        Ok(())
    }

    /// The sandbox's log since `since` (whole seconds, rounded up), at most `lines` lines.
    pub async fn sandbox_log(&self, name: &str, since: Duration, lines: u32) -> Result<String> {
        let secs = since.as_secs() + 1;
        let args = vec![
            "logs".into(),
            name.into(),
            "--source".into(),
            "sandbox".into(),
            "--since".into(),
            format!("{secs}s"),
            "-n".into(),
            lines.to_string(),
        ];
        self.checked(&args, CALL_TIMEOUT).await
    }

    /// Is the gateway reachable, and does it accept this machine's credentials?
    pub async fn status(&self) -> Result<()> {
        let status = self
            .json(&strings(&["status", "-o", "json"]), CALL_TIMEOUT)
            .await
            .context("the OpenShell gateway is not reachable")?;
        let text = status.to_string().to_ascii_lowercase();
        if text.contains("\"failed\"") || text.contains("unauthenticated") {
            bail!("the OpenShell gateway did not accept this machine's credentials: {status}");
        }
        Ok(())
    }
}

/// A policy written to a file for `--policy`, removed when dropped.
pub struct PolicyFile {
    path: std::path::PathBuf,
}

impl PolicyFile {
    pub fn write(policy: &Value) -> Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "cedar-policy-{}.yaml",
            uuid::Uuid::new_v4().simple()
        ));
        // JSON is YAML, which is what OpenShell reads.
        let text = serde_json::to_string_pretty(policy)?;
        std::fs::write(&path, text)
            .with_context(|| format!("writing the sandbox policy to {}", path.display()))?;
        Ok(Self { path })
    }

    pub fn path_string(&self) -> String {
        self.path.to_string_lossy().into_owned()
    }
}

impl Drop for PolicyFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The phase a sandbox description reports (`Ready`, `Stopped`, `Error`, ...).
pub fn phase(sandbox: &Value) -> &str {
    sandbox["phase"].as_str().unwrap_or("Unknown")
}

/// Whether an `openshell` error says the sandbox doesn't exist.
fn is_not_found(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("not found") || t.contains("notfound") || t.contains("no such sandbox")
}

/// What a failed `openshell` call printed, stderr first.
pub fn error_text(out: &std::process::Output) -> String {
    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if err.is_empty() {
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    } else {
        err
    }
}

/// The first two words of a command, for error messages (never the arguments: they can be long).
fn describe(args: &[String]) -> String {
    args.iter()
        .take(2)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

/// Docker's memory sizes (`512m`, `1g`, `1.5g`, bytes) as OpenShell's quantities (`512Mi`,
/// `1Gi`, `1536Mi`). In OpenShell's notation a bare `m` means milli, so `512m` would be half a
/// byte; the sizes cedar's settings have always used are converted rather than passed through.
pub fn memory_quantity(docker: &str) -> Result<String> {
    let s = docker.trim();
    if s.is_empty() {
        bail!("empty memory size");
    }
    // Already an OpenShell quantity.
    if s.ends_with('i') {
        return Ok(s.to_string());
    }
    let (num, unit) = match s.char_indices().find(|(_, c)| c.is_ascii_alphabetic()) {
        Some((i, _)) => (&s[..i], s[i..].to_ascii_lowercase()),
        None => (s, String::new()),
    };
    let n: f64 = num
        .parse()
        .map_err(|_| anyhow!("invalid memory size {docker:?}"))?;
    let mib = match unit.as_str() {
        "" | "b" => n / (1024.0 * 1024.0),
        "k" | "kb" => n / 1024.0,
        "m" | "mb" => n,
        "g" | "gb" => n * 1024.0,
        _ => bail!("invalid memory size {docker:?} (use e.g. 512m or 1g)"),
    };
    let mib = mib.round() as u64;
    if mib < 16 {
        bail!("memory size {docker:?} is too small for a sandbox");
    }
    Ok(format!("{mib}Mi"))
}

/// A connection OpenShell refused, from the sandbox's log.
#[derive(Debug, Clone, PartialEq)]
pub struct Denial {
    /// The program that tried, as OpenShell identified it (`NET:OPEN` lines); `None` for a
    /// refused HTTP request, whose line names only the request.
    pub binary: Option<String>,
    pub host: String,
    pub port: u16,
    /// What was refused: `connect` for a connection, or the HTTP method and path.
    pub what: String,
    /// OpenShell's reason, when it gave one.
    pub reason: Option<String>,
    /// When, in seconds since the epoch.
    pub at: f64,
}

/// The refused connections and requests in `log` (the output of `openshell logs`), in order,
/// without repeats. Lines look like:
///
/// ```text
/// [1775014132.690] [sandbox] [OCSF ] [ocsf] NET:OPEN [MED] DENIED /usr/bin/curl(64) -> httpbin.org:443 [policy:- engine:opa] [reason:no matching policy]
/// [1775014133.010] [sandbox] [OCSF ] [ocsf] HTTP:PUT [MED] DENIED PUT http://api.github.com:443/x [policy:p engine:l7]
/// ```
pub fn parse_denials(log: &str) -> Vec<Denial> {
    let mut out: Vec<Denial> = Vec::new();
    for line in log.lines() {
        let Some(denial) = parse_denial(line) else {
            continue;
        };
        let seen = out.iter().any(|d| {
            d.binary == denial.binary
                && d.host == denial.host
                && d.port == denial.port
                && d.what == denial.what
        });
        if !seen {
            out.push(denial);
        }
    }
    out
}

fn parse_denial(line: &str) -> Option<Denial> {
    if !line.contains(" DENIED ") || !line.contains("OCSF") {
        return None;
    }
    let at = line
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']'))
        .and_then(|(ts, _)| ts.trim().parse::<f64>().ok())
        .or_else(|| iso_seconds(line.split_whitespace().next()?))
        .unwrap_or(0.0);
    let reason = bracketed(line, "[reason:");
    let (class, rest) = line.split_once(" DENIED ")?;
    let class = class
        .split_whitespace()
        .find(|w| w.contains(':') && w.starts_with(|c: char| c.is_ascii_uppercase()))?;
    // What comes before the bracketed fields (`[policy:...]`, `[reason:...]`); an IPv6 address
    // has a bracket too, but not one followed by a letter.
    let end = rest
        .match_indices(" [")
        .find(|(i, _)| rest[i + 2..].starts_with(|c: char| c.is_ascii_alphabetic()))
        .map_or(rest.len(), |(i, _)| i);
    let detail = rest[..end].trim();
    if class.starts_with("NET:") {
        // `/usr/bin/curl(64) -> httpbin.org:443`
        let (who, target) = detail.split_once(" -> ")?;
        let binary = who
            .rsplit_once('(')
            .map_or(who, |(path, _)| path)
            .trim()
            .to_string();
        let (host, port) = split_host_port(target.trim())?;
        return Some(Denial {
            binary: Some(binary).filter(|b| b.starts_with('/')),
            host,
            port,
            what: "connect".into(),
            reason,
            at,
        });
    }
    if class.starts_with("HTTP:") {
        // `PUT http://api.github.com:443/x`
        let (method, url) = detail.split_once(' ')?;
        let parsed = reqwest::Url::parse(url.trim()).ok()?;
        let host = parsed.host_str()?.to_string();
        let port = parsed.port_or_known_default()?;
        return Some(Denial {
            binary: None,
            host,
            port,
            what: format!("{method} {}", parsed.path()),
            reason,
            at,
        });
    }
    None
}

/// `[reason:no matching policy]` -> `no matching policy`.
fn bracketed(line: &str, open: &str) -> Option<String> {
    let start = line.find(open)? + open.len();
    let end = line[start..].find(']')? + start;
    Some(line[start..end].trim().to_string()).filter(|s| !s.is_empty())
}

/// `host:443` or `[2001:db8::1]:443`.
fn split_host_port(target: &str) -> Option<(String, u16)> {
    let (host, port) = target.rsplit_once(':')?;
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return None;
    }
    Some((host.to_string(), port.parse().ok()?))
}

/// `2026-04-01T04:04:32.690Z` as seconds since the epoch.
fn iso_seconds(s: &str) -> Option<f64> {
    let t = chrono::DateTime::parse_from_rfc3339(s).ok()?;
    Some(t.timestamp_millis() as f64 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_memory_sizes_become_openshell_quantities() {
        for (docker, want) in [
            ("512m", "512Mi"),
            ("512M", "512Mi"),
            ("1g", "1024Mi"),
            ("1.5g", "1536Mi"),
            ("268435456", "256Mi"),
            ("2048k", "2Mi"),
            ("4Gi", "4Gi"),
            ("768Mi", "768Mi"),
        ] {
            if want == "2Mi" {
                assert!(memory_quantity(docker).is_err(), "{docker}");
                continue;
            }
            assert_eq!(memory_quantity(docker).unwrap(), want, "{docker}");
        }
        for bad in ["", "lots", "5t", "1m"] {
            assert!(memory_quantity(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn denials_are_read_from_the_log() {
        let log = "\
[1775014113.058] [sandbox] [INFO ] [openshell_sandbox] Starting sandbox
[1775014132.118] [sandbox] [OCSF ] [ocsf] NET:OPEN [INFO] ALLOWED /usr/bin/curl(58) -> api.github.com:443 [policy:github_api engine:opa]
[1775014132.690] [sandbox] [OCSF ] [ocsf] NET:OPEN [MED] DENIED /usr/bin/curl(64) -> httpbin.org:443 [policy:- engine:opa] [reason:no matching policy]
[1775014132.900] [sandbox] [OCSF ] [ocsf] NET:OPEN [MED] DENIED /usr/bin/curl(65) -> httpbin.org:443 [policy:- engine:opa] [reason:no matching policy]
[1775014133.010] [sandbox] [OCSF ] [ocsf] HTTP:PUT [MED] DENIED PUT http://api.github.com:443/repos/x [policy:gh engine:l7]
[1775014134.000] [sandbox] [OCSF ] [ocsf] NET:OPEN [MED] DENIED /usr/bin/python3.11(70) -> [2001:db8::1]:8443 [policy:- engine:opa]
[1775014135.000] [sandbox] [INFO ] [openshell_sandbox] request DENIED in upstream
";
        let denials = parse_denials(log);
        assert_eq!(denials.len(), 3, "{denials:#?}");
        assert_eq!(
            denials[0],
            Denial {
                binary: Some("/usr/bin/curl".into()),
                host: "httpbin.org".into(),
                port: 443,
                what: "connect".into(),
                reason: Some("no matching policy".into()),
                at: 1775014132.690,
            }
        );
        assert_eq!(denials[1].binary, None);
        assert_eq!(
            (
                denials[1].host.as_str(),
                denials[1].port,
                denials[1].what.as_str()
            ),
            ("api.github.com", 443, "PUT /repos/x")
        );
        assert_eq!(denials[2].host, "2001:db8::1");
        assert_eq!(denials[2].port, 8443);
        assert_eq!(denials[2].binary.as_deref(), Some("/usr/bin/python3.11"));

        // The log file's own format, with ISO timestamps.
        let file = "2026-04-01T04:04:32.690Z OCSF NET:OPEN [MED] DENIED /usr/bin/curl(64) -> httpbin.org:443 [policy:- engine:opa] [reason:no matching policy]";
        let d = &parse_denials(file)[0];
        assert_eq!(d.host, "httpbin.org");
        assert_eq!(d.binary.as_deref(), Some("/usr/bin/curl"));
        assert!((d.at - 1_775_016_272.690).abs() < 0.01, "{}", d.at);

        // As OpenShell 0.1.6 writes them: no policy field, and DNS refusals without a port.
        let real = "\
[1793252649.056] [sandbox] [OCSF ] [ocsf] NET:REFUSE [MED] DENIED example.com [reason:policy_dns_ineligible]
[1793252649.059] [sandbox] [OCSF ] [ocsf] NET:OPEN [MED] DENIED /usr/bin/python3.12(0) -> example.com:443 [reason:transparent_tcp_policy_denied]
";
        let denials = parse_denials(real);
        assert_eq!(denials.len(), 1, "{denials:#?}");
        assert_eq!(denials[0].binary.as_deref(), Some("/usr/bin/python3.12"));
        assert_eq!(
            denials[0].reason.as_deref(),
            Some("transparent_tcp_policy_denied")
        );
    }
}
