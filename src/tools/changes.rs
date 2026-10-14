//! Which workspace files a command created or modified, for the tools that run commands (`bash`
//! and `run_python`). A marker file is touched just before the command, and a second command lists
//! the workspace files that are newer, so the model, and the chat that shows the images among
//! them, can tell what the command wrote.

use super::ToolContext;
use crate::sandbox::UNLISTED_DIRS;
use anyhow::Result;
use std::time::Duration;

/// How many changed files are listed.
const MAX_LISTED: usize = 50;
const LIST_TIMEOUT: Duration = Duration::from_secs(15);

/// Touch `marker` (a path in the container's `/tmp`), to mark the moment before a command runs.
/// If this fails, [`section`] finds no marker and reports nothing.
pub(super) async fn touch(ctx: &ToolContext, marker: &str) -> Result<()> {
    let cmd = format!("touch {}", super::shell_quote(marker));
    ctx.sandbox.exec(&cmd, LIST_TIMEOUT, 1024).await?;
    Ok(())
}

/// The "files created or modified" section for what changed since `marker` was touched, or
/// nothing if there were no changes. The marker is removed.
pub(super) async fn section(ctx: &ToolContext, marker: &str) -> Result<String> {
    let listing = ctx
        .sandbox
        .exec(&changed_files_command(marker), LIST_TIMEOUT, 64 * 1024)
        .await?;
    Ok(if listing.exit_code == Some(0) {
        format_changed(&String::from_utf8_lossy(&listing.stdout))
    } else {
        String::new()
    })
}

/// List workspace files newer than the marker (skipping caches and dependency trees, like the
/// workspace listing does), one more than the cap so truncation shows, then remove the marker.
fn changed_files_command(marker: &str) -> String {
    let prune = UNLISTED_DIRS
        .iter()
        .map(|d| format!("-name {}", super::shell_quote(d)))
        .collect::<Vec<_>>()
        .join(" -o ");
    let marker = super::shell_quote(marker);
    format!(
        "[ -e {marker} ] || exit 1; find . -mindepth 1 \\( {prune} \\) -prune -o -type f -newer {marker} -print \
         | sed 's|^\\./||' | sort | head -n {}; rm -f -- {marker}",
        MAX_LISTED + 1
    )
}

/// The "files created or modified" section of the result, or nothing if there were none.
fn format_changed(listing: &str) -> String {
    let files: Vec<&str> = listing.lines().filter(|l| !l.is_empty()).collect();
    if files.is_empty() {
        return String::new();
    }
    let mut s = String::from("\n[files created or modified]");
    for f in files.iter().take(MAX_LISTED) {
        s.push_str("\n- ");
        s.push_str(f);
    }
    if files.len() > MAX_LISTED {
        s.push_str(&format!(
            "\n- … and more (only the first {MAX_LISTED} are listed)"
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_listing_prunes_noise_and_cleans_up() {
        let cmd = changed_files_command("/tmp/p.mark");
        assert!(
            cmd.starts_with(
                "[ -e '/tmp/p.mark' ] || exit 1; find . -mindepth 1 \\( -name '.git' -o "
            ),
            "{cmd}"
        );
        assert!(cmd.contains("-name 'node_modules'"), "{cmd}");
        assert!(cmd.contains("-type f -newer '/tmp/p.mark' -print"), "{cmd}");
        assert!(cmd.ends_with("head -n 51; rm -f -- '/tmp/p.mark'"), "{cmd}");
    }

    #[test]
    fn changed_files_are_listed_and_capped() {
        assert_eq!(format_changed(""), "");
        assert_eq!(
            format_changed("out/a.png\nb.csv\n"),
            "\n[files created or modified]\n- out/a.png\n- b.csv"
        );
        let many: String = (0..=MAX_LISTED).map(|i| format!("f{i}\n")).collect();
        let s = format_changed(&many);
        assert!(s.contains("- f49\n- … and more"), "{s}");
        assert!(!s.contains("- f50"), "{s}");
    }
}
