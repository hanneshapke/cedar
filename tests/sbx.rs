//! Tests of the real `sbx` sandbox backend (Docker Sandboxes, microVMs). They need `sbx`, signed
//! in, on a host with hardware virtualisation (KVM on Linux), and the sandbox image:
//!
//!     docker build -t cedar-sandbox sandbox/
//!     docker save -o cedar-sandbox.tar cedar-sandbox && sbx template load cedar-sandbox.tar
//!     cargo test --test sbx -- --ignored --test-threads 1
//!
//! Set CEDAR_TEST_SBX_BIN to use a different `sbx`, and CEDAR_TEST_SANDBOX_IMAGE a different
//! image. `--test-threads 1` because one test sweeps every cedar sandbox, as at startup.

use cedar::config::{SandboxConfig, SbxConfig};
use cedar::sandbox::{Sandbox, SandboxFactory, WorkspaceMissing};
use cedar::sbx::SbxSandboxFactory;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};
use uuid::Uuid;

const SECOND: Duration = Duration::from_secs(1);

fn sbx_bin() -> String {
    std::env::var("CEDAR_TEST_SBX_BIN").unwrap_or_else(|_| "sbx".into())
}

fn factory(network: &str) -> (SbxSandboxFactory, PathBuf) {
    let data_dir = std::env::temp_dir().join(format!("cedar-sbx-test-{}", Uuid::new_v4()));
    let cfg = SandboxConfig {
        docker_bin: "docker".into(),
        image: std::env::var("CEDAR_TEST_SANDBOX_IMAGE")
            .unwrap_or_else(|_| "cedar-sandbox:latest".into()),
        network: network.into(),
        memory: "1g".into(),
        cpus: "1".into(),
        tools_dir: None,
        tools_network: "none".into(),
    };
    let sbx = SbxConfig {
        bin: sbx_bin(),
        data_dir: data_dir.clone(),
        pull: "missing".into(),
    };
    (SbxSandboxFactory::new(cfg, sbx), data_dir)
}

/// The names `sbx ls` shows.
fn sandboxes() -> String {
    let out = Command::new(sbx_bin()).args(["ls", "--json"]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

async fn teardown(f: &SbxSandboxFactory, data_dir: PathBuf, ws: Uuid, sb: &Arc<dyn Sandbox>) {
    sb.destroy().await;
    f.remove_workspace(ws).await.unwrap();
    let _ = std::fs::remove_dir_all(data_dir);
}

#[tokio::test]
#[ignore = "needs sbx, KVM and the cedar-sandbox template"]
async fn a_run_is_its_own_microvm_and_runs_the_quick_start() {
    let (f, dir) = factory("none");
    let id = Uuid::new_v4();
    let sb = f.create(id, id).await.expect("create (is sbx signed in?)");
    assert!(sandboxes().contains(&format!("cedar-run-{id}")));

    // The README's quick start: write hello.py, run it, show the output.
    sb.write_file("hello.py", b"print('hello from the microVM')")
        .await
        .unwrap();
    let out = sb
        .exec("pwd; python3 hello.py; echo err >&2; exit 3", 30 * SECOND, 1000)
        .await
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "/workspace\nhello from the microVM\n"
    );
    assert_eq!(String::from_utf8_lossy(&out.stderr), "err\n");
    assert_eq!(out.exit_code, Some(3));

    let read = sb.read_file("hello.py", 5).await.unwrap();
    assert_eq!((read.bytes.as_slice(), read.truncated), (&b"print"[..], true));

    sb.destroy().await;
    assert!(
        !sandboxes().contains(&format!("cedar-run-{id}")),
        "removed when the run ends"
    );
    f.remove_workspace(id).await.unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
#[ignore = "needs sbx, KVM and the cedar-sandbox template"]
async fn commands_that_overrun_are_killed_and_reported_as_timed_out() {
    let (f, dir) = factory("none");
    let id = Uuid::new_v4();
    let sb = f.create(id, id).await.unwrap();
    let started = Instant::now();
    let out = sb.exec("sleep 60", 2 * SECOND, 1000).await.unwrap();
    assert_eq!(out.exit_code, Some(124));
    assert!(started.elapsed() < 15 * SECOND, "{:?}", started.elapsed());
    teardown(&f, dir, id, &sb).await;
}

#[tokio::test]
#[ignore = "needs sbx, KVM and the cedar-sandbox template"]
async fn a_continuation_sees_the_files_and_finished_workspaces_are_read_in_the_sandbox() {
    let (f, dir) = factory("none");
    let (first, ws) = (Uuid::new_v4(), Uuid::new_v4());
    let sb = f.create(first, ws).await.unwrap();
    sb.write_file("out/report.md", b"# hi").await.unwrap();
    // A link out of the workspace, and noise a listing skips.
    let secret = dir.join("secret.txt");
    std::fs::write(&secret, "host-only").unwrap();
    sb.exec(
        &format!(
            "ln -s '{}' escape && mkdir -p .git && echo g > .git/config",
            secret.display()
        ),
        30 * SECOND,
        1000,
    )
    .await
    .unwrap();
    sb.destroy().await;

    // After the run: listed and read through `cedar-files`, without following the link.
    let listing = f.list_workspace_files(ws).await.unwrap();
    let paths: Vec<_> = listing.files.iter().map(|x| x.path.as_str()).collect();
    assert_eq!(paths, ["out/report.md"], "{listing:?}");
    let file = f.read_workspace_file(ws, "out/report.md", 100).await.unwrap();
    assert_eq!(file.unwrap().bytes, b"# hi");
    assert!(f
        .read_workspace_file(ws, "escape", 100)
        .await
        .unwrap()
        .is_none());

    // A continuation (another run on the same workspace) picks the files up.
    let next = Uuid::new_v4();
    let sb = f.create(next, ws).await.unwrap();
    let out = sb.exec("cat out/report.md", 30 * SECOND, 1000).await.unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "# hi");
    assert!(f.list_workspaces().await.unwrap().contains(&ws));

    teardown(&f, dir.clone(), ws, &sb).await;
    let err = f.list_workspace_files(ws).await.unwrap_err();
    assert!(err.is::<WorkspaceMissing>(), "{err:#}");
}

#[tokio::test]
#[ignore = "needs sbx, KVM and the cedar-sandbox template"]
async fn the_default_policy_keeps_the_sandbox_offline() {
    let (f, dir) = factory("none");
    let id = Uuid::new_v4();
    let sb = f.create(id, id).await.unwrap();
    let out = sb
        .exec(
            "curl -sS --max-time 10 https://example.com >/dev/null",
            30 * SECOND,
            1000,
        )
        .await
        .unwrap();
    assert_ne!(out.exit_code, Some(0), "example.com must be unreachable");
    teardown(&f, dir, id, &sb).await;
}

#[tokio::test]
#[ignore = "needs sbx, KVM and the cedar-sandbox template"]
async fn orphans_are_swept_and_health_checks_pass() {
    let (f, dir) = factory("none");
    f.health().await.expect("sbx usable");
    let id = Uuid::new_v4();
    let _sb = f.create(id, id).await.unwrap();
    // Simulates a crash: the sandbox is never destroyed by its owner.
    assert!(f.cleanup_orphans().await.unwrap() >= 1);
    assert!(!sandboxes().contains(&format!("cedar-run-{id}")));
    // The workspace is not touched by the sweep.
    assert!(f.list_workspaces().await.unwrap().contains(&id));
    f.remove_workspace(id).await.unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
#[ignore = "needs sbx, KVM and the cedar-sandbox template"]
async fn a_personal_tool_runs_in_a_microvm_of_its_own() {
    let (f, dir) = factory("none");
    let tools = dir.join("tools");
    for name in ["echo_tool", "other_tool"] {
        std::fs::create_dir_all(tools.join(name)).unwrap();
    }
    std::fs::write(tools.join("other_tool/secret.txt"), "other").unwrap();
    std::fs::write(
        tools.join("echo_tool/tool.py"),
        "import os, sys\n\
         data = sys.stdin.read()\n\
         print(data, os.environ.get('ECHO_TOKEN'), open('/workspace/in.txt').read(), \
               os.path.exists('/opt/cedar/tool/tool.py'), os.path.exists('../other_tool'))\n",
    )
    .unwrap();
    let id = Uuid::new_v4();
    let sb = f.create(id, id).await.unwrap();
    sb.write_file("in.txt", b"from the run").await.unwrap();
    let env = vec![("ECHO_TOKEN".to_string(), "t0ken".to_string())];
    let call = cedar::sandbox::ToolRun {
        name: "echo_tool",
        dir: &tools.join("echo_tool"),
        entry: "tool.py",
        input: br#"{"q":1}"#,
        env: &env,
        hosts: &[],
    };
    let out = sb.run_tool(&call, 60 * SECOND, 1000).await.unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "{\"q\":1} t0ken from the run True False\n",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(sandboxes().contains(&format!("cedar-tool-{}-0", id.simple())));
    // The run's own sandbox never sees the tools.
    let out = sb
        .exec("ls /opt/cedar/tool 2>&1; echo $?", 30 * SECOND, 1000)
        .await
        .unwrap();
    assert!(!String::from_utf8_lossy(&out.stdout).contains("tool.py"));

    sb.destroy().await;
    assert!(
        !sandboxes().contains(&format!("cedar-tool-{}", id.simple())),
        "removed with the run"
    );
    f.remove_workspace(id).await.unwrap();
    let _ = std::fs::remove_dir_all(dir);
}
