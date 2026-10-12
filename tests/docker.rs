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
    std::env::set_var("LLM_API_KEY", "sk-should-not-leak");
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

#[tokio::test]
#[ignore = "needs Docker and the cedar-sandbox image"]
async fn workspace_files_are_listed_and_read_after_the_run_is_gone() {
    let (f, id, sb) = sandbox().await;
    sb.write_file("out/report.pdf", b"%PDF-1.4 fake")
        .await
        .unwrap();
    sb.write_file("notes/it's \"quoted\" ✓.md", b"# hi")
        .await
        .unwrap();
    // Noise that a listing should skip, and things that aren't regular files.
    sb.exec(
        "mkdir -p .git node_modules/x .local/lib && echo g > .git/config && echo n > node_modules/x/i.js \
         && echo l > .local/lib/p.py && ln -s /etc/passwd escape && ln -s out/report.pdf alias.pdf && mkdir empty",
        10 * SECOND,
        1000,
    )
    .await
    .unwrap();
    // The container is gone; the workspace volume is what gets read.
    sb.destroy().await;

    let listing = f.list_workspace_files(id).await.unwrap();
    let paths: Vec<_> = listing.files.iter().map(|x| x.path.as_str()).collect();
    assert_eq!(
        paths,
        ["notes/it's \"quoted\" ✓.md", "out/report.pdf"],
        "{listing:?}"
    );
    assert_eq!(listing.files[1].size, 13);
    assert!(!listing.truncated);
    let age = chrono::Utc::now() - listing.files[1].modified_at;
    assert!(
        age.num_seconds() < 120,
        "mtime {:?}",
        listing.files[1].modified_at
    );

    let file = f
        .read_workspace_file(id, "out/report.pdf", 1000)
        .await
        .unwrap()
        .expect("file exists");
    assert_eq!(
        (file.bytes.as_slice(), file.truncated),
        (&b"%PDF-1.4 fake"[..], false)
    );
    let short = f
        .read_workspace_file(id, "out/report.pdf", 4)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (short.bytes.as_slice(), short.truncated),
        (&b"%PDF"[..], true)
    );
    let odd = f
        .read_workspace_file(id, "notes/it's \"quoted\" ✓.md", 1000)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(odd.bytes, b"# hi");

    // Symlinks inside the workspace resolve; ones that leave it, directories, and missing files don't.
    assert!(f
        .read_workspace_file(id, "alias.pdf", 1000)
        .await
        .unwrap()
        .is_some());
    for missing in ["escape", "empty", "out", "nope.txt", ".git/config"] {
        let got = f.read_workspace_file(id, missing, 1000).await.unwrap();
        assert!(
            got.is_none() || missing == ".git/config",
            "{missing}: {got:?}"
        );
    }
    assert!(f
        .read_workspace_file(id, "../etc/passwd", 1000)
        .await
        .is_err());

    // The read-only mount must not have created anything, and the volume is untouched.
    let names = docker(&[
        "volume",
        "ls",
        "-q",
        "--filter",
        &format!("name=cedar-ws-{id}"),
    ]);
    assert_eq!(names.lines().count(), 1);

    f.remove_workspace(id).await.unwrap();
    let err = f.list_workspace_files(id).await.unwrap_err();
    assert!(
        err.downcast_ref::<cedar::sandbox::WorkspaceMissing>()
            .is_some(),
        "{err:#}"
    );
    let err = f
        .read_workspace_file(id, "out/report.pdf", 10)
        .await
        .unwrap_err();
    assert!(
        err.downcast_ref::<cedar::sandbox::WorkspaceMissing>()
            .is_some(),
        "{err:#}"
    );
    // Looking at a workspace that doesn't exist must not create a stray volume.
    assert_eq!(
        docker(&[
            "volume",
            "ls",
            "-q",
            "--filter",
            &format!("name=cedar-ws-{id}")
        ]),
        ""
    );
}

#[tokio::test]
#[ignore = "needs Docker, the cedar-sandbox image, and network access for pip"]
async fn pip_installs_into_the_workspace_and_persists_across_runs() {
    let factory = DockerSandboxFactory::new(config("bridge"));
    let id = Uuid::new_v4();
    let first = factory.create(id, id).await.unwrap();
    let out = first
        .exec(
            "pip --version && pip install --quiet six",
            120 * SECOND,
            10_000,
        )
        .await
        .unwrap();
    assert_eq!(
        out.exit_code,
        Some(0),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    first.destroy().await;

    // A continuation gets a new container on the same workspace: the package is still there.
    let second = factory.create(Uuid::new_v4(), id).await.unwrap();
    let out = second
        .exec(
            "python3 -c 'import six, os; print(six.__file__)'",
            30 * SECOND,
            1000,
        )
        .await
        .unwrap();
    let path = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.exit_code,
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(path.starts_with("/workspace/.local/"), "{path}");
    // ...and the listing doesn't drown in it.
    let listing = factory.list_workspace_files(id).await.unwrap();
    assert!(listing.files.is_empty(), "{:?}", listing.files);
    second.destroy().await;
    factory.remove_workspace(id).await.unwrap();
}

#[test]
fn sandbox_fatal_is_a_distinct_error_type() {
    // Compile-time/behavioral guard that the downcast the agent loop relies on keeps working.
    let err: anyhow::Error = SandboxFatal("x".into()).into();
    assert!(err.downcast_ref::<SandboxFatal>().is_some());
}
