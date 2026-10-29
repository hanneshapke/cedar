//! Tests of the real OpenShell runtime (opt-in; see `tests/docker.rs` for the default one). They need a running Docker daemon,
//! an OpenShell gateway the `openshell` command is registered with (its Docker driver with
//! `allow_driver_config = true`), and the sandbox image:
//!
//!     docker build -t cedar-sandbox sandbox/
//!     cargo test --test openshell -- --ignored --test-threads 4
//!
//! Set CEDAR_TEST_SANDBOX_IMAGE to use a different image.

use cedar::config::SandboxConfig;
use cedar::policy::{Endpoint, Policy};
use cedar::sandbox::{OpenShellSandboxFactory, Sandbox, SandboxFactory, SandboxFatal};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use uuid::Uuid;

fn config(policy: Policy) -> SandboxConfig {
    SandboxConfig {
        openshell_bin: "openshell".into(),
        openshell_workspace: "default".into(),
        docker_bin: "docker".into(),
        image: std::env::var("CEDAR_TEST_SANDBOX_IMAGE")
            .unwrap_or_else(|_| "cedar-sandbox:latest".into()),
        policy,
        network: "bridge".into(),
        memory: "256m".into(),
        cpus: "1".into(),
        tools_dir: None,
        tools_network: "bridge".into(),
    }
}

async fn sandbox() -> (OpenShellSandboxFactory, Uuid, Arc<dyn Sandbox>) {
    let factory = OpenShellSandboxFactory::new(config(Policy::offline()));
    let id = Uuid::new_v4();
    // A fresh run's workspace is named after the run.
    let sandbox = factory.create(id, id).await.expect(
        "create sandbox (are Docker and the OpenShell gateway running, and the image built?)",
    );
    (factory, id, sandbox)
}

fn docker(args: &[&str]) -> String {
    let out = Command::new("docker").args(args).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The sandbox's phase as OpenShell reports it, or `None` if there is no such sandbox.
fn phase(name: &str) -> Option<String> {
    phase_in("default", name)
}

fn phase_in(workspace: &str, name: &str) -> Option<String> {
    let out = Command::new("openshell")
        .args([
            "--workspace",
            workspace,
            "sandbox",
            "get",
            name,
            "-o",
            "json",
        ])
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    v["phase"].as_str().map(String::from)
}

/// Destroy the sandbox and delete its workspace volume, which `destroy` leaves behind on purpose.
async fn teardown(factory: &OpenShellSandboxFactory, id: Uuid, sandbox: &Arc<dyn Sandbox>) {
    sandbox.destroy().await;
    factory
        .remove_workspace(id)
        .await
        .expect("remove workspace");
}

const SECOND: Duration = Duration::from_secs(1);

#[tokio::test]
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
async fn a_paused_sandbox_resumes_with_its_files_and_can_still_be_destroyed() {
    let (f, id, sb) = sandbox().await;
    let name = cedar::sandbox::run_sandbox_name(id);
    let out = sb
        .exec(
            "echo kept > /tmp/scratch && echo too > kept.txt",
            10 * SECOND,
            1000,
        )
        .await
        .unwrap();
    assert_eq!(out.exit_code, Some(0));
    // OpenShell's Docker driver can't freeze a sandbox, so pausing stops it.
    sb.pause().await;
    assert_eq!(phase(&name).as_deref(), Some("Stopped"));
    sb.resume().await.unwrap();
    assert_eq!(phase(&name).as_deref(), Some("Ready"));
    let out = sb
        .exec("cat /tmp/scratch kept.txt", 10 * SECOND, 1000)
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "kept\ntoo\n",
        "the workspace and even /tmp survive a stop"
    );
    sb.resume()
        .await
        .expect("resuming a running sandbox is a no-op");

    // Paused again, it is still removed, and then can't be resumed.
    sb.pause().await;
    teardown(&f, id, &sb).await;
    assert_eq!(phase(&name), None);
    assert!(sb.resume().await.is_err());
}

#[tokio::test]
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
async fn killing_the_idle_process_doesnt_lose_the_sandbox() {
    let (f, id, sb) = sandbox().await;
    // What `pkill sleep` or `kill -9 -1` from the agent would do.
    let _ = sb.exec("pkill -9 sleep; true", 10 * SECOND, 1000).await;
    // OpenShell replaces the runtime; the next command waits for it and runs.
    let out = sb.exec("echo back", 30 * SECOND, 1000).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "back\n");
    teardown(&f, id, &sb).await;
}

#[tokio::test]
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
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
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
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
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
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
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
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
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
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
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
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
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
async fn destroy_removes_the_sandbox_but_keeps_the_workspace_and_is_idempotent() {
    let (f, id, sb) = sandbox().await;
    let name = cedar::sandbox::run_sandbox_name(id);
    let volume = format!("cedar-ws-{id}");
    assert_eq!(phase(&name).as_deref(), Some("Ready"));
    assert!(
        docker(&["volume", "ls", "-q", "--filter", &format!("name={volume}")]).contains(&volume)
    );

    sb.destroy().await;
    sb.destroy().await;
    assert_eq!(phase(&name), None);
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
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
async fn orphaned_sandboxes_are_swept_and_health_checks_pass() {
    // The sweep removes every cedar sandbox in its OpenShell workspace, so this test has one of
    // its own and leaves the other tests' sandboxes alone.
    let workspace = "cedar-sweep";
    let _ = Command::new("openshell")
        .args(["workspace", "create", "--name", workspace])
        .output();
    let factory = OpenShellSandboxFactory::new(SandboxConfig {
        openshell_workspace: workspace.into(),
        ..config(Policy::offline())
    });
    let id = Uuid::new_v4();
    let sb = factory.create(id, id).await.expect("create sandbox");
    factory
        .health()
        .await
        .expect("daemon and gateway reachable, image present");

    // Simulates a crash: the sandbox, kept stopped for an idle conversation, is never destroyed
    // by its owner.
    sb.pause().await;
    assert!(factory.cleanup_orphans().await.unwrap() >= 1);
    assert_eq!(
        phase_in(workspace, &cedar::sandbox::run_sandbox_name(id)),
        None
    );
    // Workspaces are not touched by the sweep; stray ones are removed separately at startup.
    let volume = format!("cedar-ws-{id}");
    assert!(
        docker(&["volume", "ls", "-q", "--filter", &format!("name={volume}")]).contains(&volume)
    );
    factory.remove_workspace(id).await.unwrap();
}

#[tokio::test]
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
async fn a_missing_image_is_reported_by_the_health_check_and_create() {
    let factory = OpenShellSandboxFactory::new(SandboxConfig {
        image: "cedar-no-such-image:0".into(),
        ..config(Policy::offline())
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
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
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
#[ignore = "needs Docker, an OpenShell gateway, the cedar-sandbox image, and network access for pip"]
async fn pip_installs_into_the_workspace_and_persists_across_runs() {
    let factory = OpenShellSandboxFactory::new(config(Policy::packages()));
    let id = Uuid::new_v4();
    let first = factory.create(id, id).await.unwrap();
    let out = first
        .exec(
            "pip --version && pip install --quiet colorama",
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
            "python3 -c 'import colorama; print(colorama.__file__)'",
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

#[tokio::test]
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
async fn generate_pdf_renders_markdown_in_the_sandbox() {
    use cedar::tools::{GeneratePdf, Tool, ToolContext};
    use serde_json::json;

    let (f, id, sb) = sandbox().await;
    let ctx = ToolContext {
        sandbox: sb.clone(),
        browser: None,
        timeout: 60 * SECOND,
        max_output: 32_768,
        model: None,
        locale: "en".into(),
    };
    let input = json!({
        "path": "out/it's a report.pdf",
        "title": "Quarterly",
        "content": "# Report\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n```py\nprint('hi')\n```\n\n![chart](missing.png)\n",
    });
    let msg = GeneratePdf.call(&ctx, &input).await.unwrap();
    assert!(
        msg.starts_with("wrote out/it's a report.pdf: 1 page, "),
        "{msg}"
    );
    // The missing image is reported as a warning, not a failure.
    assert!(
        msg.contains("[warnings]") && msg.contains("missing.png"),
        "{msg}"
    );

    let pdf = sb.read_file("out/it's a report.pdf", 16).await.unwrap();
    assert!(pdf.bytes.starts_with(b"%PDF-"), "{:?}", pdf.bytes);

    // Nothing but the PDF is left behind: the staged document is gone from /tmp.
    let out = sb.exec("ls /tmp", 10 * SECOND, 1000).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "");

    let html = json!({ "path": "page.pdf", "format": "html", "content": "<p>hi</p>" });
    let msg = GeneratePdf.call(&ctx, &html).await.unwrap();
    assert!(
        msg.starts_with("wrote page.pdf: 1 page, ") && msg.ends_with(" bytes"),
        "{msg}"
    );

    // A renderer failure is a tool error carrying the renderer's reason.
    let bad = json!({ "path": "/proc/x.pdf", "content": "# x" });
    let err = GeneratePdf.call(&ctx, &bad).await.unwrap_err().to_string();
    assert!(
        err.starts_with("rendering /proc/x.pdf failed (exit status 1): cannot write"),
        "{err}"
    );

    teardown(&f, id, &sb).await;
}

fn tool_ctx(sb: &Arc<dyn Sandbox>) -> cedar::tools::ToolContext {
    cedar::tools::ToolContext {
        sandbox: sb.clone(),
        browser: None,
        timeout: 60 * SECOND,
        max_output: 32_768,
        model: None,
        locale: "en".into(),
    }
}

#[tokio::test]
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
async fn bash_lists_the_files_it_created_or_modified() {
    use cedar::tools::{Bash, Tool};
    use serde_json::json;

    let (f, id, sb) = sandbox().await;
    let ctx = tool_ctx(&sb);
    sb.write_file("old.txt", b"untouched").await.unwrap();
    sb.write_file("edited.txt", b"before").await.unwrap();

    // A command that finishes at once still has its files listed; caches and untouched files don't.
    let msg = Bash
        .call(
            &ctx,
            &json!({ "command": "mkdir -p out .cache && echo x > out/fig.png && echo y > .cache/noise \
                                 && echo after > edited.txt && cat old.txt" }),
        )
        .await
        .unwrap();
    assert_eq!(
        msg,
        "untouched\n[files created or modified]\n- edited.txt\n- out/fig.png"
    );

    // A command that writes nothing reports nothing, and keeps its exit status.
    let msg = Bash
        .call(&ctx, &json!({ "command": "ls old.txt; exit 3" }))
        .await
        .unwrap();
    assert_eq!(msg, "old.txt\n\n[exit status: 3]");

    // Neither marker is left in /tmp.
    let out = sb.exec("ls -A /tmp", 10 * SECOND, 1000).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "");

    teardown(&f, id, &sb).await;
}

#[tokio::test]
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
async fn run_python_runs_code_and_lists_the_files_it_wrote() {
    use cedar::tools::{RunPython, Tool};
    use serde_json::json;

    let (f, id, sb) = sandbox().await;
    let ctx = tool_ctx(&sb);
    sb.write_file("helper.py", b"GREETING = 'hi'\n")
        .await
        .unwrap();
    sb.write_file("old.txt", b"untouched").await.unwrap();

    // Quotes and backslashes need no escaping, workspace modules import, args arrive in argv.
    let code = r#"
import os, sys
from helper import GREETING
print(GREETING, sys.argv[1:], os.getcwd(), "it's \"quoted\" \\ fine")
os.makedirs("out", exist_ok=True)
open("out/result.txt", "w").write("42")
os.makedirs(".cache", exist_ok=True)
open(".cache/noise", "w").write("x")
print("warn", file=sys.stderr)
"#;
    let msg = RunPython
        .call(&ctx, &json!({ "code": code, "args": ["a b", "c"] }))
        .await
        .unwrap();
    assert_eq!(
        msg,
        "hi ['a b', 'c'] /workspace it's \"quoted\" \\ fine\n\n[stderr]\nwarn\n\
         \n[files created or modified]\n- out/result.txt"
    );

    let msg = RunPython
        .call(&ctx, &json!({ "code": "raise SystemExit('bad input')" }))
        .await
        .unwrap();
    assert_eq!(msg, "\n[stderr]\nbad input\n\n[exit status: 1]");

    // Neither the staged script nor the marker is left in /tmp.
    let out = sb.exec("ls -A /tmp", 10 * SECOND, 1000).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "");

    teardown(&f, id, &sb).await;
}

#[tokio::test]
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
async fn read_document_extracts_office_files_csv_and_pdf() {
    use cedar::tools::{ReadDocument, RunPython, Tool};
    use serde_json::json;

    let (f, id, sb) = sandbox().await;
    let ctx = tool_ctx(&sb);
    let make = r#"
import docx, openpyxl, pptx
from pypdf import PdfWriter
d = docx.Document(); d.add_heading("Plan", 1); d.add_paragraph("Ship it.")
t = d.add_table(rows=2, cols=2); t.cell(0, 0).text = "k"; t.cell(0, 1).text = "v"
t.cell(1, 0).text = "a"; t.cell(1, 1).text = "1"; d.save("plan.docx")
wb = openpyxl.Workbook(); ws = wb.active; ws.title = "Data"; ws.append(["n", "sq"])
for i in range(1, 6): ws.append([i, i * i])
wb.save("data.xlsx")
p = pptx.Presentation(); s = p.slides.add_slide(p.slide_layouts[1])
s.shapes.title.text = "Hello"; s.placeholders[1].text = "point"; p.save("deck.pptx")
open("t.csv", "w").write("a,b\n1,2\n")
w = PdfWriter(); w.add_blank_page(100, 100); w.write("blank.pdf")
"#;
    RunPython
        .call(&ctx, &json!({ "code": make }))
        .await
        .unwrap();

    let docx = ReadDocument
        .call(&ctx, &json!({ "path": "plan.docx" }))
        .await
        .unwrap();
    assert_eq!(
        docx,
        "plan.docx: Word document, 1 table\n\n## Plan\n\nShip it.\n\n\n| k | v |\n|---|---|\n| a | 1 |\n\n"
    );
    let xlsx = ReadDocument
        .call(&ctx, &json!({ "path": "data.xlsx", "rows": "4-" }))
        .await
        .unwrap();
    assert!(
        xlsx.contains("## Sheet: Data (5 data rows; showing 2 rows of rows 4-)\n\n| n | sq |\n|---|---|\n| 4 | 16 |\n| 5 | 25 |"),
        "{xlsx}"
    );
    let pptx = ReadDocument
        .call(&ctx, &json!({ "path": "deck.pptx" }))
        .await
        .unwrap();
    assert_eq!(
        pptx,
        "deck.pptx: PowerPoint, 1 slide\n\n--- slide 1: Hello ---\n- point\n"
    );
    let csv = ReadDocument
        .call(&ctx, &json!({ "path": "t.csv" }))
        .await
        .unwrap();
    assert!(csv.contains("| a | b |\n|---|---|\n| 1 | 2 |"), "{csv}");

    // A page without text is a warning, not a failure.
    let pdf = ReadDocument
        .call(&ctx, &json!({ "path": "blank.pdf" }))
        .await
        .unwrap();
    assert!(
        pdf.starts_with("blank.pdf: PDF, 1 page\n\n--- page 1 ---\n[no text]"),
        "{pdf}"
    );
    assert!(
        pdf.contains("[warnings]\nno text could be extracted"),
        "{pdf}"
    );

    let err = ReadDocument
        .call(&ctx, &json!({ "path": "nope.pdf" }))
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(err, "nope.pdf: no such file");
    let err = ReadDocument
        .call(&ctx, &json!({ "path": "data.xlsx", "sheet": "Other" }))
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(err, "data.xlsx has no sheet 'Other'; its sheets are: Data");

    teardown(&f, id, &sb).await;
}

#[tokio::test]
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
async fn generate_chart_draws_png_svg_and_pdf() {
    use cedar::tools::{GenerateChart, Tool};
    use serde_json::json;

    let (f, id, sb) = sandbox().await;
    let ctx = tool_ctx(&sb);
    let bar = json!({ "path": "charts/sales.png", "kind": "bar", "labels": ["Q1", "Q2"],
                      "series": [{ "name": "2025", "values": [1, 2] }, { "name": "2026", "values": [2, null] }],
                      "title": "Sales" });
    let msg = GenerateChart.call(&ctx, &bar).await.unwrap();
    assert!(
        msg.starts_with("wrote charts/sales.png: ") && msg.ends_with(" bytes"),
        "{msg}"
    );
    let png = sb.read_file("charts/sales.png", 8).await.unwrap();
    assert_eq!(png.bytes, b"\x89PNG\r\n\x1a\n");

    let hist =
        json!({ "path": "h.svg", "kind": "histogram", "series": [{ "values": [1, 2, 2, 3] }] });
    GenerateChart.call(&ctx, &hist).await.unwrap();
    let svg = sb.read_file("h.svg", 200).await.unwrap();
    assert!(
        String::from_utf8_lossy(&svg.bytes).contains("<svg"),
        "{svg:?}"
    );

    let pie = json!({ "path": "p.pdf", "kind": "pie", "labels": ["a", "b"], "series": [{ "values": [1, 3] }] });
    GenerateChart.call(&ctx, &pie).await.unwrap();
    assert!(sb
        .read_file("p.pdf", 5)
        .await
        .unwrap()
        .bytes
        .starts_with(b"%PDF-"));

    // A renderer failure is a tool error carrying the reason; the staged spec is cleaned up.
    let bad = json!({ "path": "/proc/x.png", "kind": "pie", "labels": ["a"], "series": [{ "values": [1] }] });
    let err = GenerateChart
        .call(&ctx, &bad)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.starts_with("drawing /proc/x.png failed (exit status 1): cannot write"),
        "{err}"
    );
    let out = sb.exec("ls -A /tmp", 10 * SECOND, 1000).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "");

    teardown(&f, id, &sb).await;
}

#[tokio::test]
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
async fn attached_pdfs_are_saved_and_their_first_pages_rendered() {
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
    use cedar::attachments;
    use cedar::models::Attachment;

    let (f, id, sb) = sandbox().await;
    // A three-page PDF, made in the sandbox and read back as a client would send it.
    let make = "python3 -c \"from matplotlib.backends.backend_pdf import PdfPages; \
                import matplotlib.pyplot as plt; pdf = PdfPages('/tmp/three.pdf'); \
                [pdf.savefig(plt.figure()) for _ in range(3)]; pdf.close()\"";
    let out = sb.exec(make, 60 * SECOND, 1000).await.unwrap();
    assert_eq!(
        out.exit_code,
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let pdf = sb.read_file("/tmp/three.pdf", 1 << 20).await.unwrap().bytes;
    let attachment = Attachment {
        name: "three.pdf".into(),
        media_type: "application/pdf".into(),
        data: BASE64.encode(&pdf),
    };

    let blocks = attachments::prepare(sb.as_ref(), &[attachment], true, 2, 60 * SECOND)
        .await
        .unwrap();
    assert_eq!(blocks.len(), 3, "two pages and the note: {blocks:?}");
    for (n, block) in blocks[..2].iter().enumerate() {
        assert_eq!(block["title"], format!("three.pdf p.{}", n + 1));
        assert_eq!(block["source"]["media_type"], "image/jpeg");
        let jpeg = BASE64
            .decode(block["source"]["data"].as_str().unwrap())
            .unwrap();
        assert!(jpeg.starts_with(&[0xff, 0xd8]), "page {} is a JPEG", n + 1);
    }
    let note = blocks[2]["text"].as_str().unwrap();
    assert!(
        note.contains("attachments/three.pdf: PDF, 3 pages; pages 1-2 are shown"),
        "{note}"
    );

    // The PDF is in the workspace as sent, and the rendered pages are gone.
    let saved = sb
        .read_file("attachments/three.pdf", 1 << 20)
        .await
        .unwrap();
    assert_eq!(saved.bytes, pdf);
    let out = sb
        .exec("ls -d /tmp/cedar-pages-* 2>/dev/null", 10 * SECOND, 1000)
        .await
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "");

    teardown(&f, id, &sb).await;
}

#[tokio::test]
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
async fn personal_python_tools_run_in_sandboxes_of_their_own() {
    use cedar::tools::custom::SavedSecrets;
    use cedar::tools::ToolSet;
    use serde_json::json;

    // The sample tools in examples/tools, the way `TOOLS_DIR` holds them.
    let dir = std::fs::canonicalize("examples/tools").unwrap();
    let mut cfg = config(Policy::offline());
    cfg.tools_dir = Some(dir.clone());
    let factory = OpenShellSandboxFactory::new(cfg);
    let id = Uuid::new_v4();
    let sb = factory.create(id, id).await.expect("create sandbox");
    let ctx = tool_ctx(&sb);
    let mut set = ToolSet::standard();
    set.load_custom(&dir, &SavedSecrets::new()).unwrap();
    let word_count = set.get("word_count").expect("the sample tool is loaded");

    sb.write_file("notes.txt", b"one two\nthree\n")
        .await
        .unwrap();
    let msg = word_count
        .call(&ctx, &json!({ "path": "notes.txt" }))
        .await
        .unwrap();
    assert_eq!(msg.trim(), r#"{"lines": 2, "words": 3, "characters": 14}"#);

    // A failing script is a tool error carrying its stderr, and leaves nothing in /tmp.
    let err = word_count
        .call(&ctx, &json!({ "path": "missing.txt" }))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.starts_with("word_count failed (exit status 1): "),
        "{err}"
    );
    assert!(err.contains("missing.txt"), "{err}");
    let out = sb.exec("ls /tmp", 10 * SECOND, 1000).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "");

    // The run's own sandbox doesn't see the tools at all, so neither the agent nor another tool
    // can read a tool's files (or a .env next to it).
    let out = sb
        .exec("ls /opt/cedar/tools /opt/cedar/tool", 10 * SECOND, 1000)
        .await
        .unwrap();
    assert_ne!(out.exit_code, Some(0));

    factory.remove_tool_env("word_count").await.unwrap();
    teardown(&factory, id, &sb).await;
}

#[tokio::test]
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
async fn a_tool_sees_neither_other_tools_nor_their_secrets_nor_the_network() {
    use cedar::tools::custom::SavedSecrets;
    use cedar::tools::ToolSet;
    use serde_json::{json, Value};
    use std::collections::HashMap;

    let dir = std::env::temp_dir().join(format!("cedar-isolation-{}", Uuid::new_v4()));
    let write = |rel: &str, text: &str| {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    };
    // `vault` keeps a .env and has a saved secret; `spy` looks for both, and for the network.
    write(
        "vault/tool.json",
        r#"{"description": "d", "secrets": ["VAULT_KEY"]}"#,
    );
    write("vault/tool.py", "print('vault')");
    write("vault/.env", "VAULT_KEY=from-the-file");
    write(
        "spy/tool.json",
        r#"{"description": "d", "secrets": ["SPY_KEY"]}"#,
    );
    write(
        "spy/tool.py",
        &format!(
            r#"import json, os, socket
found = [p for p in ["{vault}", "/opt/cedar/tools/vault", "/opt/cedar/tool/../vault/.env"] if os.path.exists(p)]
try:
    socket.create_connection(("1.1.1.1", 53), timeout=3); net = True
except OSError:
    net = False
print(json.dumps({{"found": found, "own": sorted(os.listdir("/opt/cedar/tool")),
                  "spy_key": os.environ.get("SPY_KEY"), "vault_key": os.environ.get("VAULT_KEY"),
                  "net": net, "home": os.environ.get("HOME")}}))"#,
            vault = dir.join("vault").display()
        ),
    );
    let dir = std::fs::canonicalize(&dir).unwrap();
    // The run's own policy reaches PyPI; a tool that declares no hosts still reaches nothing.
    let mut cfg = config(Policy::packages());
    cfg.tools_dir = Some(dir.clone());
    let factory = OpenShellSandboxFactory::new(cfg);
    let id = Uuid::new_v4();
    let sb = factory.create(id, id).await.expect("create sandbox");
    let ctx = tool_ctx(&sb);
    let secrets = SavedSecrets::from([
        (
            "spy".to_string(),
            HashMap::from([("SPY_KEY".to_string(), "mine".to_string())]),
        ),
        (
            "vault".to_string(),
            HashMap::from([("VAULT_KEY".to_string(), "theirs".to_string())]),
        ),
    ]);
    let mut set = ToolSet::standard();
    set.load_custom(&dir, &secrets).unwrap();
    let out = set
        .get("spy")
        .unwrap()
        .call(&ctx, &json!({}))
        .await
        .unwrap();
    let seen: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(seen["found"], json!([]), "{seen}");
    assert_eq!(seen["own"], json!(["tool.json", "tool.py"]));
    assert_eq!(seen["spy_key"], "mine");
    assert_eq!(seen["vault_key"], Value::Null);
    assert_eq!(seen["net"], false, "no hosts declared, so no network");
    assert_eq!(seen["home"], "/opt/cedar/env");

    teardown(&factory, id, &sb).await;
    factory.remove_tool_env("spy").await.unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
async fn installing_a_tool_takes_over_root_owned_folders_in_its_environment() {
    // A volume an earlier, half-finished install left with root's `.local` and `.cache` inside.
    // It also has no OpenShell labels, as a volume from before OpenShell wouldn't.
    let factory = OpenShellSandboxFactory::new(config(Policy::offline()));
    let volume = cedar::sandbox::tool_env_volume("rooted");
    let image = config(Policy::offline()).image;
    docker(&["volume", "create", &volume]);
    docker(&[
        "run",
        "--rm",
        "--user",
        "0:0",
        "-v",
        &format!("{volume}:/opt/cedar/env"),
        &image,
        "mkdir",
        "-p",
        "/opt/cedar/env/.local/lib",
        "/opt/cedar/env/.cache/pip",
    ]);

    // pillow is in the image already, so pip needs no network to be satisfied.
    factory
        .setup_tool("rooted", &["pillow".to_string()])
        .await
        .expect("install into a volume with root-owned folders");
    let owners = docker(&[
        "run",
        "--rm",
        "-v",
        &format!("{volume}:/opt/cedar/env"),
        &image,
        "stat",
        "-c",
        "%u",
        "/opt/cedar/env/.local/lib",
        "/opt/cedar/env/.cache/pip",
    ]);
    assert_eq!(
        owners.split_whitespace().collect::<Vec<_>>(),
        ["1000", "1000"]
    );

    factory.remove_tool_env("rooted").await.unwrap();
}

#[tokio::test]
#[ignore = "needs Docker, an OpenShell gateway, the cedar-sandbox image, and network access to PyPI"]
async fn the_policy_decides_which_program_reaches_which_host_and_refusals_are_reported() {
    let factory = OpenShellSandboxFactory::new(config(Policy::packages()));
    let id = Uuid::new_v4();
    let sb = factory.create(id, id).await.expect("create sandbox");
    let since = SystemTime::now();
    let stdout =
        |out: &cedar::sandbox::ExecOutput| String::from_utf8_lossy(&out.stdout).trim().to_string();

    // Python may read from PyPI...
    let out = sb
        .exec(
            "python3 -c \"import urllib.request as u; print(u.urlopen('https://pypi.org/simple/six/', timeout=30).status)\"",
            60 * SECOND,
            1000,
        )
        .await
        .unwrap();
    assert_eq!(
        stdout(&out),
        "200",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // ...but curl may not, and nothing reaches any other host.
    let curl = "curl -sS -m 15 -o /dev/null -w '%{http_code}' https://pypi.org/simple/six/";
    let out = sb.exec(curl, 30 * SECOND, 1000).await.unwrap();
    assert_ne!(
        out.exit_code,
        Some(0),
        "curl is not allowed: {}",
        stdout(&out)
    );
    let out = sb
        .exec(
            "python3 -c \"import socket; socket.create_connection(('example.com', 443), 5)\"",
            30 * SECOND,
            1000,
        )
        .await
        .unwrap();
    assert_ne!(out.exit_code, Some(0));

    // The refusal is in the sandbox's log, with the program that tried.
    let mut denials = vec![];
    for _ in 0..20 {
        denials = sb.denials(since).await.unwrap();
        if denials
            .iter()
            .any(|d| d.binary.as_deref() == Some("/usr/bin/curl"))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let curl_denial = denials
        .iter()
        .find(|d| d.binary.as_deref() == Some("/usr/bin/curl"))
        .unwrap_or_else(|| panic!("{denials:#?}"));
    assert_eq!(
        (curl_denial.host.as_str(), curl_denial.port),
        ("pypi.org", 443)
    );

    // Allowing exactly that lets it through, live, and nothing more.
    let endpoint = Endpoint {
        binary: "/usr/bin/curl".into(),
        host: "pypi.org".into(),
        port: 443,
    };
    sb.allow(std::slice::from_ref(&endpoint)).await.unwrap();
    sb.allow(&[endpoint])
        .await
        .expect("allowing it again changes nothing");
    let out = sb.exec(curl, 30 * SECOND, 1000).await.unwrap();
    assert_eq!(
        out.exit_code,
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stdout(&out), "200");
    let out = sb
        .exec(
            "curl -sS -m 10 -o /dev/null https://example.com/",
            30 * SECOND,
            1000,
        )
        .await
        .unwrap();
    assert_ne!(out.exit_code, Some(0));

    teardown(&factory, id, &sb).await;
}

#[tokio::test]
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
async fn large_files_are_written_in_parts_and_a_workspace_from_before_openshell_is_kept() {
    // A workspace volume made before cedar ran on OpenShell has no OpenShell labels.
    let id = Uuid::new_v4();
    let volume = format!("cedar-ws-{id}");
    let image = config(Policy::offline()).image;
    docker(&[
        "volume",
        "create",
        "--label",
        &format!("cedar.workspace={id}"),
        &volume,
    ]);
    docker(&[
        "run",
        "--rm",
        "--user",
        "1000:1000",
        "-v",
        &format!("{volume}:/workspace"),
        &image,
        "sh",
        "-c",
        "echo from-before > /workspace/old.txt",
    ]);
    let factory = OpenShellSandboxFactory::new(config(Policy::offline()));
    let sb = factory
        .create(Uuid::new_v4(), id)
        .await
        .expect("create on an old workspace");
    let read = sb.read_file("old.txt", 100).await.unwrap();
    assert_eq!(read.bytes, b"from-before\n");

    // 10 MB: more than OpenShell takes in one go.
    let big: Vec<u8> = (0..10_000_000u32).map(|i| (i % 251) as u8).collect();
    sb.write_file("attachments/big.bin", &big).await.unwrap();
    let out = sb
        .exec(
            "wc -c < attachments/big.bin; ls attachments",
            10 * SECOND,
            1000,
        )
        .await
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "10000000\nbig.bin\n");
    let back = sb
        .read_file("attachments/big.bin", 20_000_000)
        .await
        .unwrap();
    assert!(back.bytes == big, "the file came back as written");

    teardown(&factory, id, &sb).await;
}

#[tokio::test]
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
async fn a_workspace_moves_between_the_runtimes_an_admin_switches() {
    use cedar::sandbox::{set_runtime, Runtime, Sandboxes};
    let store = cedar::store::Store::connect_memory().await.unwrap();
    let sandboxes = Sandboxes::new(config(Policy::offline()), store.clone());
    let ws = Uuid::new_v4();
    let read = |sb: Arc<dyn Sandbox>| async move {
        let out = sb.exec("cat notes.txt", 10 * SECOND, 1000).await.unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    // Docker by default.
    assert_eq!(sandboxes.runtime().await, Runtime::Docker);
    let first = sandboxes.create(Uuid::new_v4(), ws).await.unwrap();
    assert_eq!(first.runtime(), Runtime::Docker);
    first.write_file("notes.txt", b"from docker").await.unwrap();
    first.destroy().await;

    // The same workspace, now in an OpenShell sandbox.
    set_runtime(&store, Some(Runtime::OpenShell)).await.unwrap();
    sandboxes.check_runtime(Runtime::OpenShell).await.unwrap();
    let second = sandboxes.create(Uuid::new_v4(), ws).await.unwrap();
    assert_eq!(second.runtime(), Runtime::OpenShell);
    assert_eq!(read(second.clone()).await, "from docker");
    second.write_file("notes.txt", b"from openshell").await.unwrap();
    second.destroy().await;

    // And back.
    set_runtime(&store, None).await.unwrap();
    let third = sandboxes.create(Uuid::new_v4(), ws).await.unwrap();
    assert_eq!(third.runtime(), Runtime::Docker);
    assert_eq!(read(third.clone()).await, "from openshell");
    third.destroy().await;
    sandboxes.remove_workspace(ws).await.unwrap();
}

#[tokio::test]
#[ignore = "needs Docker, an OpenShell gateway, and the cedar-sandbox image"]
async fn an_image_that_works_in_the_workspace_is_refused_with_the_reason() {
    // Images from before OpenShell (0.2.3 and earlier, as published) work in /workspace, which
    // OpenShell reserves for the workspace volume.
    let base = config(Policy::offline()).image;
    let dir = std::env::temp_dir().join(format!("cedar-oldimg-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Dockerfile"), format!("FROM {base}\nWORKDIR /workspace\n")).unwrap();
    let tag = "cedar-sandbox:test-old-workdir";
    let built = Command::new("docker")
        .args(["build", "-q", "-t", tag])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(built.status.success(), "{}", String::from_utf8_lossy(&built.stderr));
    let factory = OpenShellSandboxFactory::new(SandboxConfig {
        image: tag.into(),
        ..config(Policy::offline())
    });
    let err = format!("{:#}", factory.health().await.unwrap_err());
    assert!(err.contains("works in /workspace") && err.contains("sandbox/Dockerfile"), "{err}");
    let id = Uuid::new_v4();
    let err = format!("{:#}", factory.create(id, id).await.err().unwrap());
    assert!(err.contains("works in /workspace"), "{err}");
    let _ = Command::new("docker").args(["image", "rm", tag]).output();
    let _ = std::fs::remove_file(dir.join("Dockerfile"));
    let _ = std::fs::remove_dir(&dir);
}
