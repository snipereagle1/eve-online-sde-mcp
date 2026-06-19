use anyhow::{Context, Result, bail};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use reqwest::blocking::Client;
use serde_json::Value;
use std::{
    fs,
    io::{self, BufRead, Read, Write},
    path::{Component, Path, PathBuf},
};
use zip::ZipArchive;

use crate::config::{Config, Meta};

const SDE_URL: &str =
    "https://developers.eveonline.com/static-data/eve-online-static-data-latest-jsonl.zip";

pub(crate) struct DownloadResult {
    pub(crate) build: u64,
    pub(crate) release_date: String,
    pub(crate) was_downloaded: bool,
}

pub(crate) fn check_and_update(cfg: &Config) -> Result<DownloadResult> {
    let client = Client::builder()
        .user_agent("eve-sde-mcp/0.1")
        .build()
        .context("build HTTP client")?;

    let (build, etag, final_url) = head_check(&client)?;

    let meta_path = cfg.meta_path();
    let existing = Meta::load(&meta_path)?;

    if !cfg.redownload
        && let Some(ref meta) = existing
        && meta.build == build
    {
        return Ok(DownloadResult {
            build,
            release_date: meta.release_date.clone(),
            was_downloaded: false,
        });
    }

    let data_dir = cfg.resolved_data_dir();
    fs::create_dir_all(&data_dir).context("create data dir")?;

    let zip_tmp = data_dir.join(format!("sde-{build}.zip.tmp"));
    download_zip(&client, &final_url, &zip_tmp)?;

    let sde_dir = cfg.sde_dir(build);
    extract_zip(&zip_tmp, &sde_dir)?;
    fs::remove_file(&zip_tmp).context("remove temp zip")?;

    let release_date = read_release_date(&sde_dir)?;

    let meta = Meta {
        build,
        release_date: release_date.clone(),
        etag: etag.clone(),
    };
    meta.save(&meta_path)?;

    for stale in cfg.stale_sde_dirs(build)? {
        fs::remove_dir_all(&stale)
            .with_context(|| format!("remove stale dir {}", stale.display()))?;
    }

    Ok(DownloadResult {
        build,
        release_date,
        was_downloaded: true,
    })
}

fn head_check(client: &Client) -> Result<(u64, String, String)> {
    let resp = client
        .head(SDE_URL)
        .send()
        .context("HEAD request to SDE URL failed")?;

    if !resp.status().is_success() && resp.status().as_u16() != 405 {
        bail!("HEAD returned unexpected status: {}", resp.status());
    }

    let final_url = resp.url().as_str().to_owned();
    let build = parse_build(&final_url)?;
    let etag = resp
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();

    Ok((build, etag, final_url))
}

pub fn parse_build(url: &str) -> Result<u64> {
    const PREFIX: &str = "eve-online-static-data-";
    let start = url
        .find(PREFIX)
        .with_context(|| format!("build prefix not found in URL: {url}"))?
        + PREFIX.len();
    let rest = &url[start..];
    let end = rest
        .find('-')
        .with_context(|| format!("expected '-' after build number in URL: {url}"))?;
    rest[..end].parse::<u64>().context("parse build number")
}

fn download_zip(client: &Client, url: &str, dest: &Path) -> Result<()> {
    let mut resp = client.get(url).send().context("GET zip failed")?;

    if !resp.status().is_success() {
        bail!("GET zip returned status: {}", resp.status());
    }

    let content_length = resp.content_length().unwrap_or(0);

    let pb = ProgressBar::with_draw_target(
        Some(content_length).filter(|&n| n > 0),
        ProgressDrawTarget::stderr(),
    );
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})",
        )
        .unwrap()
        .progress_chars("#>-"),
    );

    let mut file = fs::File::create(dest).with_context(|| format!("create {}", dest.display()))?;

    let mut downloaded = 0u64;
    let mut buf = vec![0u8; 65536];
    loop {
        let n = resp.read(&mut buf).context("read response body")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).context("write zip chunk")?;
        downloaded += n as u64;
        pb.set_position(downloaded);
    }
    pb.finish_with_message("downloaded");

    if content_length > 0 && downloaded != content_length {
        bail!("content-length mismatch: expected {content_length}, got {downloaded}");
    }

    Ok(())
}

fn extract_zip(zip_path: &Path, dest: &Path) -> Result<()> {
    let file = fs::File::open(zip_path).with_context(|| format!("open {}", zip_path.display()))?;
    let mut archive = ZipArchive::new(file).context("open zip archive")?;

    fs::create_dir_all(dest).with_context(|| format!("create {}", dest.display()))?;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).context("read zip entry")?;
        let raw_name = entry.name().to_owned();

        if raw_name.starts_with("__MACOSX/") {
            continue;
        }

        let rel = sanitize_zip_path(&raw_name)?;
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out_path = dest.join(&rel);

        if !out_path.starts_with(dest) {
            bail!("zip path traversal detected: {raw_name}");
        }

        if entry.is_dir() {
            fs::create_dir_all(&out_path)
                .with_context(|| format!("create zip dir {}", out_path.display()))?;
        } else {
            if let Some(parent) = out_path.parent() {
                fs::create_dir_all(parent)
                    .with_context(|| format!("create parent {}", parent.display()))?;
            }
            let mut out = fs::File::create(&out_path)
                .with_context(|| format!("create {}", out_path.display()))?;
            io::copy(&mut entry, &mut out).with_context(|| format!("extract {raw_name}"))?;
        }
    }

    Ok(())
}

fn sanitize_zip_path(raw: &str) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for component in Path::new(raw).components() {
        match component {
            Component::Normal(c) => out.push(c),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) | Component::ParentDir => {
                bail!("unsafe path component in zip entry: {raw}");
            }
        }
    }
    Ok(out)
}

fn find_sde_meta(dir: &Path) -> Result<PathBuf> {
    let direct = dir.join("_sde.jsonl");
    if direct.exists() {
        return Ok(direct);
    }
    for entry in fs::read_dir(dir).context("read sde dir")? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let nested = entry.path().join("_sde.jsonl");
            if nested.exists() {
                return Ok(nested);
            }
        }
    }
    bail!("_sde.jsonl not found under {}", dir.display())
}

fn read_release_date(sde_dir: &Path) -> Result<String> {
    let path = find_sde_meta(sde_dir)?;
    let file = fs::File::open(&path).with_context(|| format!("open {}", path.display()))?;
    let reader = io::BufReader::new(file);
    for line in reader.lines() {
        let line = line.context("read _sde.jsonl line")?;
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = serde_json::from_str(&line).context("parse _sde.jsonl line")?;
        if let Some(date) = v.get("releaseDate").and_then(|d| d.as_str()) {
            return Ok(date.to_owned());
        }
    }
    bail!("releaseDate not found in _sde.jsonl")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_build_extracts_number_from_redirect_url() {
        let url = "https://data.eveonline.com/tranquility/eve-online-static-data-3333874-jsonl.zip";
        assert_eq!(parse_build(url).unwrap(), 3333874);
    }

    #[test]
    fn parse_build_returns_error_when_prefix_missing() {
        let url = "https://data.eveonline.com/tranquility/something-else.zip";
        assert!(parse_build(url).is_err());
    }

    #[test]
    fn parse_build_returns_error_when_no_trailing_dash() {
        let url = "https://data.eveonline.com/tranquility/eve-online-static-data-3333874.zip";
        assert!(parse_build(url).is_err());
    }

    #[test]
    fn sanitize_zip_path_rejects_parent_traversal() {
        assert!(sanitize_zip_path("../escape/path").is_err());
    }

    #[test]
    fn sanitize_zip_path_strips_curdir() {
        let p = sanitize_zip_path("./foo/bar.txt").unwrap();
        assert_eq!(p, PathBuf::from("foo/bar.txt"));
    }

    #[test]
    fn sanitize_zip_path_accepts_normal_path() {
        let p = sanitize_zip_path("sde/types.jsonl").unwrap();
        assert_eq!(p, PathBuf::from("sde/types.jsonl"));
    }

    /// Requires network. Run with: cargo test -- --ignored
    #[test]
    #[ignore]
    fn integration_head_check_returns_pinned_build_or_newer() {
        let pinned = crate::sde_version::PINNED_BUILD;
        let client = Client::builder()
            .user_agent("eve-sde-mcp/0.1")
            .build()
            .unwrap();
        let (build, _etag, _url) = head_check(&client).unwrap();
        assert!(build >= pinned, "expected build >= {pinned}, got {build}");
    }
}
