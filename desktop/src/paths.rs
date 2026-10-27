//! Where the app keeps its files. Tauri derives both directories from the bundle identifier:
//! `~/Library/Application Support/io.github.hanneshapke.cedar/` and `~/Library/Logs/io.github.hanneshapke.cedar/`.
//! Personal tools live in `~/.cedar/tools`, where people put their own and where tools installed
//! from the registry go.

use anyhow::{Context, Result};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Clone, Debug)]
pub struct Paths {
    /// Config and the two SQLite databases.
    pub data: PathBuf,
    /// `cedar.log` and `cedar-chat.log`.
    pub logs: PathBuf,
    /// Personal tools (cedar's `TOOLS_DIR`): `~/.cedar/tools`.
    pub tools: PathBuf,
}

impl Paths {
    pub fn resolve(app: &AppHandle) -> Result<Self> {
        let data = app.path().app_data_dir().context("no application support directory")?;
        let logs = app.path().app_log_dir().context("no logs directory")?;
        std::fs::create_dir_all(&data).with_context(|| format!("creating {}", data.display()))?;
        std::fs::create_dir_all(&logs).with_context(|| format!("creating {}", logs.display()))?;
        let tools = app.path().home_dir().context("no home directory")?.join(".cedar").join("tools");
        std::fs::create_dir_all(&tools).with_context(|| format!("creating {}", tools.display()))?;
        Ok(Self { data, logs, tools })
    }

    pub fn config(&self) -> PathBuf {
        self.data.join("config.json")
    }

    pub fn cedar_db(&self) -> PathBuf {
        self.data.join("cedar.db")
    }

    pub fn chat_db(&self) -> PathBuf {
        self.data.join("cedar-chat.db")
    }

    pub fn cedar_log(&self) -> PathBuf {
        self.logs.join("cedar.log")
    }

    pub fn chat_log(&self) -> PathBuf {
        self.logs.join("cedar-chat.log")
    }
}
