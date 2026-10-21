//! What starting another program takes on each platform: finding its executable (`docker` on a
//! Mac, `docker.exe` on Windows) and, on Windows, keeping it from opening a console window. The app
//! is a GUI program there with no console of its own, so every console program it starts would
//! otherwise flash one up.

use std::path::{Path, PathBuf};

/// `CREATE_NO_WINDOW`, from the Windows process creation flags.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `name` in `dir`, with the platform's executable suffix, if it is there and can be run.
pub fn executable_in(dir: &Path, name: &str) -> Option<PathBuf> {
    let path = dir.join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    is_executable(&path).then_some(path)
}

pub fn is_executable(p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        p.is_file()
    }
}

/// A command that opens no console window.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> tokio::process::Command {
    #[allow(unused_mut)]
    let mut cmd = tokio::process::Command::new(program);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// [`command`], for the few places that wait synchronously.
pub fn std_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executables_are_found_with_the_platform_suffix() {
        let dir = std::env::temp_dir().join(format!("cedar-proc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("tool{}", std::env::consts::EXE_SUFFIX));
        std::fs::write(&file, b"").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(executable_in(&dir, "tool"), None, "not executable yet");
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(executable_in(&dir, "tool"), Some(file));
        assert_eq!(executable_in(&dir, "other"), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
