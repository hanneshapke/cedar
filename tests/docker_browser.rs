//! Tests of the real browser container. They need a running Docker daemon and the browser image:
//!
//!     docker build -t cedar-browser browser/
//!     cargo test --test docker_browser -- --ignored
//!
//! Set CEDAR_TEST_BROWSER_IMAGE to use a different image. The pages are served from inside the
//! browser container (on its own loopback), so the tests need no internet access.

use cedar::browser::{Browser, BrowserFactory, BrowserLost, DockerBrowserFactory};
use cedar::config::BrowserConfig;
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const T: Duration = Duration::from_secs(60);

const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Gorge Retreat</title></head>
<body><h1>Gorge Retreat</h1><p>Rated 5.0 · 12 reviews</p>
<button id="open">Show all 12 reviews</button>
<script>
document.getElementById("open").onclick = () => {
  const d = document.createElement("div");
  d.setAttribute("role", "dialog"); d.setAttribute("aria-modal", "true");
  d.innerHTML = "<h2>Reviews</h2><button aria-label='Close'>x</button><p>Great stay, very clean.</p>";
  document.body.appendChild(d);
};
</script></body></html>"#;

fn config() -> BrowserConfig {
    BrowserConfig {
        enabled: true,
        image: std::env::var("CEDAR_TEST_BROWSER_IMAGE")
            .unwrap_or_else(|_| "cedar-browser:latest".into()),
        network: "bridge".into(),
        memory: "1g".into(),
        cpus: "1".into(),
    }
}

fn docker(args: &[&str]) -> String {
    let out = Command::new("docker").args(args).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn container(id: Uuid) -> String {
    format!("cedar-browser-{id}")
}

/// Start a browser and serve [`PAGE`] inside its container at http://127.0.0.1:8000/.
async fn browser() -> (DockerBrowserFactory, Uuid, Arc<dyn Browser>) {
    let factory = DockerBrowserFactory::new(config());
    let id = Uuid::new_v4();
    let browser = factory
        .create(id)
        .await
        .expect("create browser (is Docker running and the image built?)");
    let name = container(id);
    let mut write = Command::new("docker")
        .args([
            "exec",
            "-i",
            &name,
            "sh",
            "-c",
            "mkdir -p /tmp/site && cat > /tmp/site/index.html",
        ])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    write
        .stdin
        .take()
        .unwrap()
        .write_all(PAGE.as_bytes())
        .unwrap();
    assert!(write.wait().unwrap().success());
    docker(&[
        "exec",
        "-d",
        &name,
        "python3",
        "-m",
        "http.server",
        "8000",
        "--bind",
        "127.0.0.1",
        "--directory",
        "/tmp/site",
    ]);
    (factory, id, browser)
}

/// Navigate, retrying while the page server inside the container is still starting.
async fn open_page(browser: &Arc<dyn Browser>) -> Value {
    let request = json!({ "action": "navigate", "url": "http://127.0.0.1:8000/" });
    for _ in 0..20 {
        match browser.call(&request, T).await {
            Ok(reply) => return reply,
            Err(e) if e.to_string().contains("ERR_CONNECTION_REFUSED") => {
                tokio::time::sleep(Duration::from_millis(250)).await
            }
            Err(e) => panic!("navigate failed: {e:#}"),
        }
    }
    panic!("the page server never came up");
}

#[tokio::test]
#[ignore = "needs Docker and the cedar-browser image"]
async fn pages_are_read_and_clicked_and_the_container_is_removed() {
    let (factory, id, browser) = browser().await;

    let page = open_page(&browser).await;
    assert_eq!(page["status"], 200);
    assert_eq!(page["title"], "Gorge Retreat");
    let text = page["text"].as_str().unwrap();
    assert!(text.contains("# Gorge Retreat"), "{text}");
    assert!(
        text.contains("[1] button \"Show all 12 reviews\""),
        "{text}"
    );

    // The page survives between calls: the click acts on what the navigation loaded.
    let dialog = browser
        .call(&json!({ "action": "click", "ref": 1 }), T)
        .await
        .unwrap();
    let text = dialog["text"].as_str().unwrap();
    assert!(text.starts_with("[a dialog is open"), "{text}");
    assert!(text.contains("Great stay, very clean."), "{text}");
    assert!(
        !text.contains("Rated 5.0"),
        "only the dialog is shown: {text}"
    );

    factory.destroy(id).await;
    factory.destroy(id).await;
    assert_eq!(
        docker(&[
            "ps",
            "-a",
            "--filter",
            &format!("name={}", container(id)),
            "--format",
            "{{.Names}}"
        ]),
        ""
    );
}

#[tokio::test]
#[ignore = "needs Docker and the cedar-browser image"]
async fn driver_errors_come_back_as_messages_and_the_browser_keeps_going() {
    let (factory, id, browser) = browser().await;
    open_page(&browser).await;

    let err = browser
        .call(&json!({ "action": "click", "ref": 42 }), T)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("[42] is not on the current page"), "{err}");
    let err = browser
        .call(
            &json!({ "action": "navigate", "url": "file:///etc/passwd" }),
            T,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("not an http or https URL"), "{err}");

    let again = browser
        .call(&json!({ "action": "snapshot" }), T)
        .await
        .unwrap();
    assert_eq!(again["title"], "Gorge Retreat");
    factory.destroy(id).await;
}

#[tokio::test]
#[ignore = "needs Docker and the cedar-browser image"]
async fn the_container_is_locked_down() {
    let (factory, id, _browser) = browser().await;
    let name = container(id);
    assert_eq!(docker(&["exec", &name, "id", "-u"]), "1000");
    assert_eq!(
        docker(&["inspect", "--format", "{{.HostConfig.CapDrop}}", &name]),
        "[ALL]"
    );
    let env = docker(&["exec", &name, "env"]);
    assert!(
        !env.contains("CEDAR_API_KEYS") && !env.contains("LLM_API_KEY"),
        "{env}"
    );
    factory.destroy(id).await;
}

#[tokio::test]
#[ignore = "needs Docker and the cedar-browser image"]
async fn a_browser_that_died_is_reported_as_lost() {
    let (factory, id, browser) = browser().await;
    docker(&["kill", &container(id)]);
    let err = browser
        .call(&json!({ "action": "snapshot" }), T)
        .await
        .unwrap_err();
    assert!(err.is::<BrowserLost>(), "{err:#}");
    factory.destroy(id).await;
}

#[tokio::test]
#[ignore = "needs Docker and the cedar-browser image"]
async fn orphans_are_swept_and_a_missing_image_is_reported() {
    let (factory, id, _browser) = browser().await;
    factory.health().await.expect("the image is present");
    // Simulates a crash: the browser is never destroyed by its run.
    assert!(factory.cleanup_orphans().await.unwrap() >= 1);
    assert_eq!(
        docker(&[
            "ps",
            "-a",
            "--filter",
            &format!("name={}", container(id)),
            "--format",
            "{{.Names}}"
        ]),
        ""
    );

    let missing = DockerBrowserFactory::new(BrowserConfig {
        image: "cedar-no-such-image:0".into(),
        ..config()
    });
    let err = format!("{:#}", missing.health().await.unwrap_err());
    assert!(err.contains("docker build"), "{err}");
    let id = Uuid::new_v4();
    assert!(missing.create(id).await.is_err());
}
