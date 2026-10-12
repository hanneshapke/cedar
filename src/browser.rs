//! A headless browser per run, for the `browser` tool.
//!
//! The browser runs in its own container (the `cedar-browser` image, see `browser/`), separate
//! from the run's sandbox: Chromium needs network access and a few hundred MB that most runs never
//! use. It is started on the run's first `browser` call and removed with the run. Each call is a
//! `docker exec` of `cedar-browser call <json>`, which talks to the long-running driver inside the
//! container, so the page, its dialogs, and its history last from one call to the next.

use crate::config::BrowserConfig;
use crate::sandbox::{docker_checked, docker_output, DOCKER_CALL_TIMEOUT, DOCKER_CREATE_TIMEOUT};
use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// A browser that is running, for one run.
#[async_trait]
pub trait Browser: Send + Sync {
    /// Send one request (see `browser/driver.py`) and return the driver's reply. An `Err` carries
    /// the driver's explanation, meant for the model; [`BrowserLost`] means the browser itself is
    /// stuck or gone.
    async fn call(&self, request: &Value, timeout: Duration) -> Result<Value>;
}

#[async_trait]
pub trait BrowserFactory: Send + Sync {
    /// Start a browser for `run_id`.
    async fn create(&self, run_id: Uuid) -> Result<Arc<dyn Browser>>;
    /// Remove `run_id`'s browser, including one whose `create` was interrupted. Best effort and
    /// idempotent: failures are logged, not returned.
    async fn destroy(&self, run_id: Uuid);
    /// Remove browsers left behind by a previous process. Returns how many were removed.
    async fn cleanup_orphans(&self) -> Result<usize>;
    /// Is the browser image available? (for `/readyz`)
    async fn health(&self) -> Result<()>;
}

/// The browser hung or stopped, e.g. Chromium ran out of memory. The session replaces it, since
/// a call that never returned may still be holding the driver.
#[derive(Debug, thiserror::Error)]
#[error("the browser {0}")]
pub struct BrowserLost(pub String);

/// A run's browser: started on first use, restarted if it stops responding, and destroyed with
/// the run.
pub struct BrowserSession {
    factory: Arc<dyn BrowserFactory>,
    run_id: Uuid,
    browser: tokio::sync::Mutex<Option<Arc<dyn Browser>>>,
    /// A browser was (or may have been) created, so `destroy` has something to remove.
    started: AtomicBool,
}

impl BrowserSession {
    pub fn new(factory: Arc<dyn BrowserFactory>, run_id: Uuid) -> Self {
        Self {
            factory,
            run_id,
            browser: tokio::sync::Mutex::new(None),
            started: AtomicBool::new(false),
        }
    }

    /// Send one request, starting the browser first if this is the run's first call.
    pub async fn call(&self, request: &Value, timeout: Duration) -> Result<Value> {
        let browser = {
            let mut slot = self.browser.lock().await;
            match slot.as_ref() {
                Some(browser) => browser.clone(),
                None => {
                    self.started.store(true, Ordering::SeqCst);
                    let browser = match self.factory.create(self.run_id).await {
                        Ok(browser) => browser,
                        Err(e) => {
                            // The detail (daemon socket paths, image names, ...) is for operators.
                            tracing::error!(run_id = %self.run_id, error = %format!("{e:#}"), "could not start the browser");
                            bail!("the browser could not be started");
                        }
                    };
                    *slot = Some(browser.clone());
                    browser
                }
            }
        };
        match browser.call(request, timeout).await {
            Err(e) if e.is::<BrowserLost>() => {
                self.browser.lock().await.take();
                self.factory.destroy(self.run_id).await;
                Err(anyhow!(
                    "{e}, so it was closed; the next call starts a new browser (navigate again: \
                     the page and its history are gone)"
                ))
            }
            other => other,
        }
    }

    /// Remove the browser if one was started. Safe to call more than once.
    pub async fn destroy(&self) {
        if self.started.swap(false, Ordering::SeqCst) {
            self.browser.lock().await.take();
            self.factory.destroy(self.run_id).await;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Docker
// ---------------------------------------------------------------------------------------------

const LABEL: &str = "cedar.browser";
/// The most of a reply that is read; snapshots are far smaller, this only guards against a
/// runaway page.
const MAX_REPLY_BYTES: usize = 16 * 1024 * 1024;

fn container_name(run_id: Uuid) -> String {
    format!("cedar-browser-{run_id}")
}

/// Arguments for `docker run` that start a run's browser. The image's command starts the driver.
pub fn run_args(cfg: &BrowserConfig, run_id: Uuid) -> Vec<String> {
    [
        "run",
        "-d",
        "--init", // Chromium starts many processes; reap them
        "--name",
        &container_name(run_id),
        "--label",
        &format!("{LABEL}={run_id}"),
        "--network",
        &cfg.network,
        "--memory",
        &cfg.memory,
        "--memory-swap",
        &cfg.memory,
        "--cpus",
        &cfg.cpus,
        "--pids-limit",
        "512",
        "--shm-size",
        "256m",
        "--cap-drop",
        "ALL",
        "--security-opt",
        "no-new-privileges",
        "--user",
        "1000:1000",
        &cfg.image,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Arguments for `docker exec` that send `request` to the driver. The JSON is one argv element.
pub fn call_args(container: &str, request: &Value) -> Vec<String> {
    vec![
        "exec".into(),
        container.into(),
        "cedar-browser".into(),
        "call".into(),
        request.to_string(),
    ]
}

pub struct DockerBrowser {
    docker: String,
    container: String,
}

#[async_trait]
impl Browser for DockerBrowser {
    async fn call(&self, request: &Value, timeout: Duration) -> Result<Value> {
        let args = call_args(&self.container, request);
        // docker_output's own limit is only a backstop; this one is the call's.
        let call = docker_output(&self.docker, &args, timeout + DOCKER_CALL_TIMEOUT);
        let out = tokio::time::timeout(timeout, call)
            .await
            .map_err(|_| BrowserLost(format!("did not respond within {}s", timeout.as_secs())))??;
        if !out.status.success() {
            let message = String::from_utf8_lossy(&out.stderr).trim().to_string();
            if message.contains("is not running") || message.contains("No such container") {
                return Err(BrowserLost("stopped (it may have run out of memory)".into()).into());
            }
            bail!(if message.is_empty() {
                format!("the browser call failed ({})", out.status)
            } else {
                message
            });
        }
        if out.stdout.len() > MAX_REPLY_BYTES {
            bail!("the page is too large to describe");
        }
        serde_json::from_slice(&out.stdout).context("the browser sent an unreadable reply")
    }
}

pub struct DockerBrowserFactory {
    docker: String,
    cfg: BrowserConfig,
}

impl DockerBrowserFactory {
    pub fn new(cfg: BrowserConfig) -> Self {
        Self {
            docker: "docker".into(),
            cfg,
        }
    }
}

#[async_trait]
impl BrowserFactory for DockerBrowserFactory {
    async fn create(&self, run_id: Uuid) -> Result<Arc<dyn Browser>> {
        if let Err(e) = docker_checked(
            &self.docker,
            &run_args(&self.cfg, run_id),
            DOCKER_CREATE_TIMEOUT,
        )
        .await
        {
            self.destroy(run_id).await;
            return Err(e.context("creating browser container"));
        }
        Ok(Arc::new(DockerBrowser {
            docker: self.docker.clone(),
            container: container_name(run_id),
        }))
    }

    async fn destroy(&self, run_id: Uuid) {
        let container = container_name(run_id);
        let args = ["rm", "-f", &container].map(String::from);
        if let Err(e) = docker_checked(&self.docker, &args, DOCKER_CALL_TIMEOUT).await {
            // Removing a container that never got created is fine.
            if !e.to_string().contains("No such container") {
                tracing::warn!(error = %e, %container, "removing browser container failed");
            }
        }
    }

    async fn cleanup_orphans(&self) -> Result<usize> {
        let ps = ["ps", "-aq", "--filter", &format!("label={LABEL}")].map(String::from);
        let ids = docker_checked(&self.docker, &ps, DOCKER_CALL_TIMEOUT).await?;
        let ids: Vec<String> = ids.split_whitespace().map(String::from).collect();
        if !ids.is_empty() {
            let mut args = vec!["rm".to_string(), "-f".into()];
            args.extend(ids.iter().cloned());
            docker_checked(&self.docker, &args, DOCKER_CALL_TIMEOUT).await?;
        }
        Ok(ids.len())
    }

    async fn health(&self) -> Result<()> {
        let inspect =
            ["image", "inspect", "--format", "{{.Id}}", &self.cfg.image].map(String::from);
        docker_checked(&self.docker, &inspect, DOCKER_CALL_TIMEOUT)
            .await
            .with_context(|| {
                format!(
                    "browser image {} is missing; build it with `docker build -t {} browser/`",
                    self.cfg.image, self.cfg.image
                )
            })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;

    fn cfg() -> BrowserConfig {
        BrowserConfig {
            enabled: true,
            image: "browser:1".into(),
            network: "bridge".into(),
            memory: "1g".into(),
            cpus: "1".into(),
        }
    }

    #[test]
    fn run_args_lock_the_container_down() {
        let id = Uuid::nil();
        let args = run_args(&cfg(), id);
        let joined = args.join(" ");
        assert!(joined.contains(&format!("--name cedar-browser-{id}")));
        assert!(joined.contains(&format!("--label cedar.browser={id}")));
        assert!(joined.contains("--network bridge"));
        assert!(joined.contains("--cap-drop ALL"));
        assert!(joined.contains("--security-opt no-new-privileges"));
        assert!(joined.contains("--user 1000:1000"));
        assert!(joined.contains("--memory 1g --memory-swap 1g"));
        assert!(!joined.contains("-v "), "the browser gets no workspace");
        // The image comes last, so its own command (the driver) runs.
        assert_eq!(args.last().unwrap(), "browser:1");
    }

    #[test]
    fn call_args_keep_the_request_as_a_single_argv_element() {
        let request = json!({ "action": "type", "ref": 3, "text": "a'b; $(rm -rf /)" });
        let args = call_args("c1", &request);
        assert_eq!(args.len(), 5);
        assert_eq!(&args[..4], ["exec", "c1", "cedar-browser", "call"]);
        assert_eq!(serde_json::from_str::<Value>(&args[4]).unwrap(), request);
    }

    /// Replies `{"n": <call number>}`, or hangs when the request says so.
    #[derive(Default)]
    struct FakeBrowser {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl Browser for FakeBrowser {
        async fn call(&self, request: &Value, timeout: Duration) -> Result<Value> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
            match request["action"].as_str() {
                Some("hang") => Err(BrowserLost(format!(
                    "did not respond within {}s",
                    timeout.as_secs()
                ))
                .into()),
                Some("fail") => bail!("[9] is not on the current page"),
                _ => Ok(json!({ "n": n })),
            }
        }
    }

    #[derive(Default)]
    struct FakeFactory {
        created: Mutex<Vec<Uuid>>,
        destroyed: Mutex<Vec<Uuid>>,
        fail_create: AtomicBool,
    }

    #[async_trait]
    impl BrowserFactory for FakeFactory {
        async fn create(&self, run_id: Uuid) -> Result<Arc<dyn Browser>> {
            if self.fail_create.load(Ordering::SeqCst) {
                bail!("dial unix /var/run/docker.sock: no such file");
            }
            self.created.lock().unwrap().push(run_id);
            Ok(Arc::new(FakeBrowser::default()))
        }
        async fn destroy(&self, run_id: Uuid) {
            self.destroyed.lock().unwrap().push(run_id);
        }
        async fn cleanup_orphans(&self) -> Result<usize> {
            Ok(0)
        }
        async fn health(&self) -> Result<()> {
            Ok(())
        }
    }

    const T: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn the_browser_starts_on_first_use_and_is_reused() {
        let factory = Arc::new(FakeFactory::default());
        let run = Uuid::new_v4();
        let session = BrowserSession::new(factory.clone(), run);
        session.destroy().await;
        assert!(
            factory.destroyed.lock().unwrap().is_empty(),
            "nothing to remove before first use"
        );

        let first = session.call(&json!({ "action": "snapshot" }), T).await;
        let second = session.call(&json!({ "action": "snapshot" }), T).await;
        assert_eq!(first.unwrap()["n"], 1);
        assert_eq!(second.unwrap()["n"], 2, "the same browser answers");
        assert_eq!(*factory.created.lock().unwrap(), [run]);

        session.destroy().await;
        session.destroy().await;
        assert_eq!(*factory.destroyed.lock().unwrap(), [run]);
    }

    #[tokio::test]
    async fn driver_errors_reach_the_model_and_keep_the_browser() {
        let factory = Arc::new(FakeFactory::default());
        let session = BrowserSession::new(factory.clone(), Uuid::new_v4());
        let err = session
            .call(&json!({ "action": "fail" }), T)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "[9] is not on the current page");
        session
            .call(&json!({ "action": "snapshot" }), T)
            .await
            .unwrap();
        assert_eq!(factory.created.lock().unwrap().len(), 1);
        assert!(factory.destroyed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unresponsive_browser_is_replaced() {
        let factory = Arc::new(FakeFactory::default());
        let session = BrowserSession::new(factory.clone(), Uuid::new_v4());
        let err = session
            .call(&json!({ "action": "hang" }), T)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("did not respond within 5s"), "{err}");
        assert!(err.contains("navigate again"), "{err}");
        assert_eq!(factory.destroyed.lock().unwrap().len(), 1);

        let reply = session.call(&json!({ "action": "snapshot" }), T).await;
        assert_eq!(reply.unwrap()["n"], 1, "a fresh browser");
        assert_eq!(factory.created.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn start_failures_hide_infrastructure_details_but_are_cleaned_up() {
        let factory = Arc::new(FakeFactory::default());
        factory.fail_create.store(true, Ordering::SeqCst);
        let session = BrowserSession::new(factory.clone(), Uuid::new_v4());
        let err = session
            .call(&json!({ "action": "snapshot" }), T)
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(err, "the browser could not be started");
        session.destroy().await;
        assert_eq!(
            factory.destroyed.lock().unwrap().len(),
            1,
            "a half-created container is removed"
        );
    }
}
