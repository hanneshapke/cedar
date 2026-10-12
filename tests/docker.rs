//! Tests of the real Docker sandbox. They need a running Docker daemon and the sandbox image:
//!
//!     docker build -t cedar-sandbox sandbox/
//!     cargo test --test docker -- --ignored
//!
//! Set CEDAR_TEST_SANDBOX_IMAGE to use a different image.

use cedar::config::SandboxConfig;
use cedar::sandbox::{DockerSandboxFactory, Sandbox, SandboxFactory, SandboxFatal};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

fn config(network: &str) -> SandboxConfig {
    SandboxConfig {
        image: std::env::var("CEDAR_TEST_SANDBOX_IMAGE")
            .unwrap_or_else(|_| "cedar-sandbox:latest".into()),
        network: network.into(),
        memory: "256m".into(),
        cpus: "1".into(),
    }
}

async fn sandbox() -> (DockerSandboxFactory, Uuid, Arc<dyn Sandbox>) {
    let factory = DockerSandboxFactory::new(config("none"));
    let id = Uuid::new_v4();
    // A fresh run's workspace is named after the run.
    let sandbox = factory
        .create(id, id)
        .await
        .expect("create sandbox (is Docker running and the image built?)");
    (factory, id, sandbox)
}

fn docker(args: &[&str]) -> String {
    let out = Command::new("docker").args(args).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Destroy the sandbox and delete its workspace volume, which `destroy` leaves behind on purpose.
async fn teardown(factory: &DockerSandboxFactory, id: Uuid, sandbox: &Arc<dyn Sandbox>) {
    sandbox.destroy().await;
    factory
        .remove_workspace(id)
        .await
        .expect("remove workspace");
}

const SECOND: Duration = Duration::from_secs(1);

#[tokio::test]
#[ignore = "needs Docker and the cedar-sandbox image"]
async fn exec_captures_output_exit_code_and_working_directory() {
    let (f, id, sb) = sandbox().await;
    let out = sb
        .exec("pwd; echo out; echo err >&2; exit 3", 10 * SECOND, 1000)
        .await
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "/workspace\nout\n");
    assert_eq!(String::from_utf8_lossy(&out.stderr), "err\n");
    assert_eq!(out.exit_code, Some(3));
    teardown(&f, id, &sb).await;
}

#[tokio::test]
#[ignore = "needs Docker and the cedar-sandbox image"]
async fn commands_that_overrun_are_killed_and_reported_as_timed_out() {
    let (f, id, sb) = sandbox().await;
    let started = Instant::now();
    let out = sb.exec("sleep 60", 2 * SECOND, 1000).await.unwrap();
    assert_eq!(out.exit_code, Some(124));
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "took {:?}",
        started.elapsed()
    );

    // The sandbox is still usable afterwards.
    let out = sb.exec("echo alive", 10 * SECOND, 1000).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "alive\n");
    teardown(&f, id, &sb).await;
}

#[tokio::test]
#[ignore = "needs Docker and the cedar-sandbox image"]
async fn runaway_output_is_capped_without_exhausting_memory() {
    let (f, id, sb) = sandbox().await;
    let out = sb
        .exec("head -c 5000000 /dev/zero | tr '\\0' x", 30 * SECOND, 1000)
        .await
        .unwrap();
    assert_eq!(out.stdout.len(), 1000);
    assert_eq!(out.stdout_total, 5_000_000);

    // An endless producer is stopped by the timeout, not by filling the host's memory.
    let out = sb.exec("yes", 2 * SECOND, 1000).await.unwrap();
    assert_eq!(out.exit_code, Some(124));
    assert_eq!(out.stdout.len(), 1000);
    teardown(&f, id, &sb).await;
}

#[tokio::test]
#[ignore = "needs Docker and the cedar-sandbox image"]
async fn files_round_trip_through_the_sandbox() {
    let (f, id, sb) = sandbox().await;
    let text = "héllo\nwörld ✓\n";
    sb.write_file("deep/nested/dir/a.txt", text.as_bytes())
        .await
        .unwrap();

    let read = sb.read_file("deep/nested/dir/a.txt", 1000).await.unwrap();
    assert_eq!(String::from_utf8(read.bytes).unwrap(), text);
    assert!(!read.truncated);

    // The same file is visible to commands, and relative paths resolve against /workspace.
    let out = sb
        .exec("cat /workspace/deep/nested/dir/a.txt", 10 * SECOND, 1000)
        .await
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), text);

    let short = sb.read_file("deep/nested/dir/a.txt", 4).await.unwrap();
    assert_eq!((short.bytes.len(), short.truncated), (4, true));

    // Paths that need quoting are handled, not interpreted.
    sb.write_file("it's a $(echo hacked) file.txt", b"x")
        .await
        .unwrap();
    let out = sb.exec("ls", 10 * SECOND, 1000).await.unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("it's a $(echo hacked) file.txt"));

    let err = sb
        .read_file("missing.txt", 100)
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("No such file"), "{err}");
    let err = sb.read_file("deep", 100).await.unwrap_err().to_string();
    assert!(err.contains("directory"), "{err}");
    teardown(&f, id, &sb).await;
}

#[tokio::test]
#[ignore = "needs Docker and the cedar-sandbox image"]
async fn the_sandbox_cannot_see_the_host_even_through_symlinks() {
    let (f, id, sb) = sandbox().await;
    let dir = std::env::temp_dir().join(format!("cedar-host-secret-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let secret = dir.join("secret.txt");
    std::fs::write(&secret, "host-only").unwrap();
    let secret = secret.to_str().unwrap();

    assert!(
        sb.read_file(secret, 100).await.is_err(),
        "host path must not exist in the sandbox"
    );
    sb.exec(&format!("ln -s '{secret}' link"), 10 * SECOND, 1000)
        .await
        .unwrap();
    assert!(
        sb.read_file("link", 100).await.is_err(),
        "a symlink must not lead to the host"
    );

    std::fs::remove_dir_all(dir).unwrap();
    teardown(&f, id, &sb).await;
}

#[tokio::test]
#[ignore = "needs Docker and the cedar-sandbox image"]
async fn the_sandbox_is_unprivileged_offline_and_free_of_secrets() {
    std::env::set_var("ANTHROPIC_API_KEY", "sk-should-not-leak");
    let (f, id, sb) = sandbox().await;

    let out = sb.exec("id -u", 10 * SECOND, 100).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "1000");

    let out = sb.exec("env", 10 * SECOND, 10_000).await.unwrap();
    assert!(!String::from_utf8_lossy(&out.stdout).contains("sk-should-not-leak"));

    let out = sb
        .exec(
            "python3 -c \"import socket; socket.create_connection(('1.1.1.1', 53), 3)\"",
            15 * SECOND,
            1000,
        )
        .await
        .unwrap();
    assert_ne!(
        out.exit_code,
        Some(0),
        "network must be disabled by default"
    );

    let out = sb
        .exec("touch /etc/cedar-test", 10 * SECOND, 1000)
        .await
        .unwrap();
    assert_ne!(
        out.exit_code,
        Some(0),
        "the agent must not be able to write system files"
    );
    teardown(&f, id, &sb).await;
}

#[tokio::test]
#[ignore = "needs Docker and the cedar-sandbox image"]
async fn destroy_removes_the_container_but_keeps_the_workspace_and_is_idempotent() {
    let (f, id, sb) = sandbox().await;
    let name = format!("cedar-run-{id}");
    let volume = format!("cedar-ws-{id}");
    assert!(docker(&[
        "ps",
        "-a",
        "--filter",
        &format!("name={name}"),
        "--format",
        "{{.Names}}"
    ])
    .contains(&name));
    assert!(
        docker(&["volume", "ls", "-q", "--filter", &format!("name={volume}")]).contains(&volume)
    );

    sb.destroy().await;
    sb.destroy().await;
    assert_eq!(
        docker(&[
            "ps",
            "-a",
            "--filter",
            &format!("name={name}"),
            "--format",
            "{{.Names}}"
        ]),
        ""
    );
    // The workspace outlives the run so a continuation can mount it; removing it is explicit.
    assert!(
        docker(&["volume", "ls", "-q", "--filter", &format!("name={volume}")]).contains(&volume)
    );
    f.remove_workspace(id).await.unwrap();
    f.remove_workspace(id).await.unwrap();
    assert_eq!(
        docker(&["volume", "ls", "-q", "--filter", &format!("name={volume}")]),
        ""
    );
}

#[tokio::test]
#[ignore = "needs Docker and the cedar-sandbox image"]
async fn orphaned_sandboxes_are_swept_and_health_checks_pass() {
    let (factory, id, _sb) = sandbox().await;
    factory
        .health()
        .await
        .expect("daemon reachable and image present");

    // Simulates a crash: the sandbox is never destroyed by its owner.
    assert!(factory.cleanup_orphans().await.unwrap() >= 1);
    assert_eq!(
        docker(&[
            "ps",
            "-a",
            "--filter",
            &format!("name=cedar-run-{id}"),
            "--format",
            "{{.Names}}"
        ]),
        ""
    );
    // Workspaces are not touched by the sweep; stray ones are removed separately at startup.
    let volume = format!("cedar-ws-{id}");
    assert!(
        docker(&["volume", "ls", "-q", "--filter", &format!("name={volume}")]).contains(&volume)
    );
    factory.remove_workspace(id).await.unwrap();
}

#[tokio::test]
#[ignore = "needs Docker and the cedar-sandbox image"]
async fn a_missing_image_is_reported_by_the_health_check_and_create() {
    let factory = DockerSandboxFactory::new(SandboxConfig {
        image: "cedar-no-such-image:0".into(),
        ..config("none")
    });
    let err = format!("{:#}", factory.health().await.unwrap_err());
    assert!(err.contains("docker build"), "{err}");
    let id = Uuid::new_v4();
    assert!(factory.create(id, id).await.is_err());
    // The workspace volume is created before the container, so it can outlive a failed create;
    // stray workspaces are swept at startup. Remove it so the test leaves nothing behind.
    factory.remove_workspace(id).await.unwrap();
}

#[test]
fn sandbox_fatal_is_a_distinct_error_type() {
    // Compile-time/behavioral guard that the downcast the agent loop relies on keeps working.
    let err: anyhow::Error = SandboxFatal("x".into()).into();
    assert!(err.downcast_ref::<SandboxFatal>().is_some());
}
