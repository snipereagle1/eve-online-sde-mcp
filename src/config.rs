use anyhow::{Context, Result};
use clap::Parser;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(about = "EVE Online SDE MCP server")]
pub(crate) struct Config {
    /// Data directory for SDE files. Optional. Precedence: this flag, then the
    /// SDE_DATA_DIR env var (an empty value is treated as unset), then a per-OS
    /// default (Linux: ~/.local/share/eve-sde-mcp, Windows: %APPDATA%\eve-sde-mcp,
    /// macOS: ~/Library/Application Support/eve-sde-mcp).
    #[arg(long, env = "SDE_DATA_DIR")]
    pub(crate) data_dir: Option<PathBuf>,

    /// Log level (overrides RUST_LOG)
    #[arg(long, default_value = "warn")]
    pub(crate) log_level: String,

    /// Force re-download even if SDE is current
    #[arg(long, default_value_t = false)]
    pub(crate) redownload: bool,

    /// Language for localized names (e.g. "en", "de"). Defaults to "en" to reduce token usage.
    #[arg(long, env = "SDE_LANGUAGE", default_value = "en")]
    pub(crate) language: Option<String>,
}

impl Config {
    /// Resolve the SDE data directory (per the precedence on [`Config::data_dir`])
    /// and ensure it exists on disk. Errors if no directory can be determined.
    pub(crate) fn resolved_data_dir(&self) -> Result<PathBuf> {
        let dir = self.data_dir_setting()?;
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("create SDE data dir {}", dir.display()))?;
        Ok(dir)
    }

    /// Pure precedence resolution with no filesystem side effects: the `--data-dir`
    /// flag / `SDE_DATA_DIR` env var (empty treated as unset), else the per-OS default.
    fn data_dir_setting(&self) -> Result<PathBuf> {
        if let Some(d) = self.data_dir.as_ref().filter(|d| !d.as_os_str().is_empty()) {
            return Ok(d.clone());
        }
        default_data_dir()
    }

    pub(crate) fn sde_dir(&self, build: u64) -> Result<PathBuf> {
        Ok(self.resolved_data_dir()?.join(format!("sde-{build}")))
    }

    pub(crate) fn meta_path(&self) -> Result<PathBuf> {
        Ok(self.resolved_data_dir()?.join("meta.json"))
    }

    pub(crate) fn stale_sde_dirs(&self, current_build: u64) -> Result<Vec<PathBuf>> {
        let base = self.resolved_data_dir()?;
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

/// Per-OS default data dir via the `directories` crate:
/// Linux `~/.local/share/eve-sde-mcp`, Windows `%APPDATA%\eve-sde-mcp\data`,
/// macOS `~/Library/Application Support/eve-sde-mcp`.
fn default_data_dir() -> Result<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "eve-sde-mcp").context(
        "no home directory found for the default SDE data dir; \
         pass --data-dir or set SDE_DATA_DIR",
    )?;
    Ok(dirs.data_dir().to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_data_dir(data_dir: Option<&str>) -> Config {
        Config {
            data_dir: data_dir.map(PathBuf::from),
            log_level: "warn".to_string(),
            redownload: false,
            language: Some("en".to_string()),
        }
    }

    #[test]
    fn empty_data_dir_falls_back_to_per_os_default() {
        // MCPB injects SDE_DATA_DIR="" when the optional user_config knob is
        // left unset, so an empty path must defer to the per-OS default.
        let cfg = config_with_data_dir(Some(""));
        assert_eq!(cfg.data_dir_setting().unwrap(), default_data_dir().unwrap());
    }

    #[test]
    fn unset_data_dir_uses_per_os_default() {
        let cfg = config_with_data_dir(None);
        assert_eq!(cfg.data_dir_setting().unwrap(), default_data_dir().unwrap());
    }

    #[test]
    fn explicit_data_dir_is_honored() {
        let cfg = config_with_data_dir(Some("/custom/sde"));
        assert_eq!(
            cfg.data_dir_setting().unwrap(),
            PathBuf::from("/custom/sde")
        );
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Meta {
    pub(crate) build: u64,
    pub(crate) release_date: String,
    pub(crate) etag: String,
}

impl Meta {
    pub(crate) fn load(path: &Path) -> Result<Option<Self>> {
        match std::fs::read_to_string(path) {
            Ok(s) => {
                let meta = serde_json::from_str(&s).context("parse meta.json")?;
                Ok(Some(meta))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).context("read meta.json"),
        }
    }

    pub(crate) fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("create data dir")?;
        }
        let s = serde_json::to_string_pretty(self).context("serialize meta")?;
        std::fs::write(path, s).context("write meta.json")
    }
}
