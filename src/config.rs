use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(about = "EVE Online SDE MCP server")]
pub struct Config {
    /// Data directory for SDE files
    #[arg(long, env = "SDE_DATA_DIR")]
    pub data_dir: Option<PathBuf>,

    /// Log level (overrides RUST_LOG)
    #[arg(long, default_value = "warn")]
    pub log_level: String,

    /// Force re-download even if SDE is current
    #[arg(long, default_value_t = false)]
    pub redownload: bool,

    /// Language for localized names (e.g. "en", "de")
    #[arg(long, env = "SDE_LANGUAGE")]
    pub language: Option<String>,
}

#[allow(dead_code)]
impl Config {
    pub fn resolved_data_dir(&self) -> PathBuf {
        if let Some(ref d) = self.data_dir {
            return d.clone();
        }
        default_data_dir()
    }

    pub fn sde_dir(&self, build: u64) -> PathBuf {
        self.resolved_data_dir().join(format!("sde-{build}"))
    }

    pub fn meta_path(&self) -> PathBuf {
        self.resolved_data_dir().join("meta.json")
    }

    pub fn stale_sde_dirs(&self, current_build: u64) -> Result<Vec<PathBuf>> {
        let base = self.resolved_data_dir();
        if !base.exists() {
            return Ok(vec![]);
        }
        let mut stale = vec![];
        for entry in std::fs::read_dir(&base).context("read data dir")? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(build_str) = name.strip_prefix("sde-")
                && let Ok(build) = build_str.parse::<u64>()
                && build != current_build
            {
                stale.push(entry.path());
            }
        }
        Ok(stale)
    }
}

#[allow(dead_code)]
fn default_data_dir() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            return PathBuf::from(appdata).join("eve-sde-mcp");
        }
    }
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        return PathBuf::from(xdg).join("eve-sde-mcp");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".local/share/eve-sde-mcp")
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct Meta {
    pub build: u64,
    pub release_date: String,
    pub etag: String,
}

#[allow(dead_code)]
impl Meta {
    pub fn load(path: &Path) -> Result<Option<Self>> {
        match std::fs::read_to_string(path) {
            Ok(s) => {
                let meta = serde_json::from_str(&s).context("parse meta.json")?;
                Ok(Some(meta))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).context("read meta.json"),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("create data dir")?;
        }
        let s = serde_json::to_string_pretty(self).context("serialize meta")?;
        std::fs::write(path, s).context("write meta.json")
    }
}
