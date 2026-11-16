//! Tests of the real computer container. They need a running Docker daemon and the computer
//! image:
//!
//!     docker build -t cedar-computer computer/
//!     cargo test --test docker_computer -- --ignored
//!
//! Set CEDAR_TEST_COMPUTER_IMAGE to use a different image. The desktop runs without a network.

use base64::Engine;
use cedar::computer::{Computer, ComputerFactory, DockerComputerFactory};
use cedar::config::ComputerConfig;
use serde_json::{json, Value};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

const T: Duration = Duration::from_secs(90);

fn config() -> ComputerConfig {
    ComputerConfig {
        enabled: true,
        docker_bin: "docker".into(),
        image: std::env::var("CEDAR_TEST_COMPUTER_IMAGE")
            .unwrap_or_else(|_| "cedar-computer:latest".into()),
        network: "none".into(),
        memory: "2g".into(),
        cpus: "2".into(),
        width: 1024,
        height: 768,
    }
}

fn docker(args: &[&str]) -> String {
    let out = Command::new("docker").args(args).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn container(run: Uuid) -> String {
    format!("cedar-computer-{run}")
}

/// A desktop on a workspace of its own; the caller removes both with [`remove`].
async fn computer() -> (DockerComputerFactory, Uuid, Uuid, Arc<dyn Computer>) {
    let factory = DockerComputerFactory::new(config());
    let (run, workspace) = (Uuid::new_v4(), Uuid::new_v4());
    let computer = factory
        .create(run, workspace)
        .await
        .expect("create computer (is Docker running and the image built?)");
    (factory, run, workspace, computer)
}

async fn remove(factory: &DockerComputerFactory, run: Uuid, workspace: Uuid) {
    factory.destroy(run).await;
    docker(&["volume", "rm", "-f", &format!("cedar-ws-{workspace}")]);
}

/// The size of the screenshot in `reply`, which must be a PNG.
fn screenshot_size(reply: &Value) -> (u32, u32) {
    let png = base64::engine::general_purpose::STANDARD
        .decode(reply["image"].as_str().expect("an image"))
        .unwrap();
    let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
    (image.width(), image.height())
}

#[tokio::test]
#[ignore = "needs Docker and the cedar-computer image"]
async fn the_desktop_comes_up_and_shows_its_screen() {
    let (factory, run, workspace, computer) = computer().await;

    let reply = computer.call(&json!({ "action": "screenshot" }), T).await.unwrap();
    assert_eq!(reply["screen"], json!([1024, 768]), "{}", without_image(&reply));
    assert_eq!(screenshot_size(&reply), (1024, 768));

    // The pointer goes where it is sent, and a smaller screenshot can be asked for.
    let moved = computer
        .call(&json!({ "action": "move", "x": 300, "y": 200, "width": 512, "height": 384 }), T)
        .await
        .unwrap();
    assert_eq!(moved["pointer"], json!([300, 200]));
    assert_eq!(screenshot_size(&moved), (512, 384));

    // A request the driver can't carry out is explained, and the desktop stays.
    let err = computer
        .call(&json!({ "action": "click", "x": 5000, "y": 10 }), T)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("`x` must be"), "{err}");
    computer.call(&json!({ "action": "screenshot" }), T).await.unwrap();

    let user = docker(&["exec", &container(run), "id", "-u"]);
    assert_eq!(user, "1000", "the desktop runs unprivileged");

    remove(&factory, run, workspace).await;
    assert_eq!(docker(&["ps", "-aq", "--filter", &format!("name={}", container(run))]), "");
}

#[tokio::test]
#[ignore = "needs Docker and the cedar-computer image"]
async fn what_an_application_saves_is_in_the_workspace() {
    let (factory, run, workspace, computer) = computer().await;
    let name = container(run);
    docker(&["exec", &name, "sh", "-c", "printf 'first line\\n' > /workspace/note.txt"]);

    // Open the file in the text editor, add a line at its end, and save with the keyboard.
    let opened = computer
        .call(&json!({ "action": "open", "path": "note.txt" }), T)
        .await
        .unwrap();
    let mut window = opened["window"].as_str().unwrap_or_default().to_string();
    for _ in 0..10 {
        if window.contains("note.txt") {
            break;
        }
        let waited = computer.call(&json!({ "action": "wait", "seconds": 1 }), T).await.unwrap();
        window = waited["window"].as_str().unwrap_or_default().to_string();
    }
    assert!(window.contains("note.txt"), "the editor has the keyboard: {window:?}");
    for request in [
        json!({ "action": "key", "keys": "ctrl+End" }),
        json!({ "action": "type", "text": "typed on the desktop\n" }),
        json!({ "action": "key", "keys": "ctrl+s" }),
        json!({ "action": "wait", "seconds": 1 }),
    ] {
        computer.call(&request, T).await.unwrap();
    }
    let saved = docker(&["exec", &name, "cat", "/workspace/note.txt"]);
    assert_eq!(saved, "first line\ntyped on the desktop");

    // Nothing outside the workspace can be opened.
    let err = computer
        .call(&json!({ "action": "open", "path": "../etc/passwd" }), T)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("inside the workspace"), "{err}");

    remove(&factory, run, workspace).await;
}

#[tokio::test]
#[ignore = "needs Docker and the cedar-computer image"]
async fn leftover_desktops_are_removed_at_startup() {
    let (factory, run, workspace, _computer) = computer().await;
    assert!(factory.cleanup_orphans().await.unwrap() >= 1);
    assert_eq!(docker(&["ps", "-aq", "--filter", &format!("name={}", container(run))]), "");
    factory.health().await.unwrap();
    remove(&factory, run, workspace).await;
}

/// `reply` without its screenshot, for an assertion's message.
fn without_image(reply: &Value) -> Value {
    let mut reply = reply.clone();
    reply.as_object_mut().map(|r| r.remove("image"));
    reply
}
