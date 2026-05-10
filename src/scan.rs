use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use memchr::memmem;
use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::store::{SdeIndex, SdeStore};

const SDE_FILE_COUNT: u64 = 16;

pub fn scan_sde(sde_dir: &Path, build: u64, release_date: &str) -> Result<Arc<SdeStore>> {
    let root = find_sde_root(sde_dir)?;

    let pb = ProgressBar::with_draw_target(Some(SDE_FILE_COUNT), ProgressDrawTarget::stderr());
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}",
        )
        .unwrap()
        .progress_chars("#>-"),
    );

    let types = scan_index(&root.join("types.jsonl"), &pb)?;
    let groups = scan_index(&root.join("groups.jsonl"), &pb)?;
    let categories = scan_index(&root.join("categories.jsonl"), &pb)?;
    let (blueprints, product_to_blueprint) =
        scan_blueprints(&root.join("blueprints.jsonl"), &pb)?;
    let type_materials = scan_index(&root.join("typeMaterials.jsonl"), &pb)?;
    let map_solar_systems = scan_index(&root.join("mapSolarSystems.jsonl"), &pb)?;
    let map_constellations = scan_index(&root.join("mapConstellations.jsonl"), &pb)?;
    let map_regions = scan_index(&root.join("mapRegions.jsonl"), &pb)?;
    let stargate_graph = scan_stargates(&root.join("mapStargates.jsonl"), &pb)?;
    let npc_stations = scan_index(&root.join("npcStations.jsonl"), &pb)?;
    let market_groups = scan_index(&root.join("marketGroups.jsonl"), &pb)?;
    let dogma_attributes = scan_index(&root.join("dogmaAttributes.jsonl"), &pb)?;
    let dogma_effects = scan_index(&root.join("dogmaEffects.jsonl"), &pb)?;
    let factions = scan_index(&root.join("factions.jsonl"), &pb)?;
    let npc_corporations = scan_index(&root.join("npcCorporations.jsonl"), &pb)?;
    let skins = scan_index(&root.join("skins.jsonl"), &pb)?;

    pb.finish_with_message("done");

    tracing::debug!(
        "SDE scan complete: {} files, build {}",
        SDE_FILE_COUNT,
        build
    );

    Ok(Arc::new(SdeStore {
        data_dir: sde_dir.to_path_buf(),
        build,
        release_date: release_date.to_owned(),
        files_scanned: SDE_FILE_COUNT as usize,
        last_updated: release_date.to_owned(),
        types,
        groups,
        categories,
        blueprints,
        type_materials,
        map_solar_systems,
        map_constellations,
        map_regions,
        npc_stations,
        market_groups,
        dogma_attributes,
        dogma_effects,
        factions,
        npc_corporations,
        skins,
        product_to_blueprint,
        stargate_graph,
    }))
}

fn find_sde_root(sde_dir: &Path) -> Result<PathBuf> {
    if sde_dir.join("_sde.jsonl").exists() {
        return Ok(sde_dir.to_path_buf());
    }
    for entry in fs::read_dir(sde_dir).context("read sde dir")? {
        let entry = entry?;
        if entry.file_type()?.is_dir() && entry.path().join("_sde.jsonl").exists() {
            return Ok(entry.path());
        }
    }
    anyhow::bail!("_sde.jsonl not found under {}", sde_dir.display())
}

#[cfg(test)]
pub fn scan_index_pub(path: &Path, pb: &ProgressBar) -> Result<SdeIndex> {
    scan_index(path, pb)
}

#[cfg(test)]
pub fn scan_blueprints_pub(
    path: &Path,
    pb: &ProgressBar,
) -> Result<(SdeIndex, HashMap<u64, u64>)> {
    scan_blueprints(path, pb)
}

fn scan_index(path: &Path, pb: &ProgressBar) -> Result<SdeIndex> {
    pb.set_message(
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    );

    let file =
        fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::with_capacity(65536, file);
    let mut id_index = HashMap::new();
    let mut name_index = HashMap::new();
    let mut line = Vec::new();
    let mut offset = 0u64;

    loop {
        let line_start = offset;
        line.clear();
        let n = reader
            .read_until(b'\n', &mut line)
            .with_context(|| format!("read {}", path.display()))?;
        if n == 0 {
            break;
        }
        offset += n as u64;

        let trimmed = line.trim_ascii();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(key) = extract_key(trimmed) {
            id_index.insert(key, line_start);
        }
        if let Some(name) = extract_name_en(trimmed) {
            name_index.insert(name.to_lowercase(), line_start);
        }
    }

    pb.inc(1);
    Ok(SdeIndex {
        path: path.to_path_buf(),
        id_index,
        name_index,
    })
}

fn scan_blueprints(
    path: &Path,
    pb: &ProgressBar,
) -> Result<(SdeIndex, HashMap<u64, u64>)> {
    pb.set_message(
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    );

    #[derive(serde::Deserialize)]
    struct Line {
        #[serde(rename = "_key")]
        key: u64,
        activities: Option<Activities>,
    }
    #[derive(serde::Deserialize)]
    struct Activities {
        manufacturing: Option<Manufacturing>,
    }
    #[derive(serde::Deserialize)]
    struct Manufacturing {
        products: Option<Vec<Product>>,
    }
    #[derive(serde::Deserialize)]
    struct Product {
        #[serde(rename = "typeID")]
        type_id: u64,
    }

    let file =
        fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::with_capacity(65536, file);
    let mut id_index = HashMap::new();
    let mut product_to_blueprint = HashMap::new();
    let mut buf = String::new();
    let mut offset = 0u64;

    loop {
        let line_start = offset;
        buf.clear();
        let n = reader
            .read_line(&mut buf)
            .with_context(|| format!("read {}", path.display()))?;
        if n == 0 {
            break;
        }
        offset += n as u64;

        let trimmed = buf.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(parsed) = serde_json::from_str::<Line>(trimmed) {
            id_index.insert(parsed.key, line_start);
            if let Some(acts) = parsed.activities
                && let Some(mfg) = acts.manufacturing
                && let Some(mut products) = mfg.products
                && !products.is_empty()
            {
                let product = products.swap_remove(0);
                product_to_blueprint.insert(product.type_id, parsed.key);
            }
        }
    }

    pb.inc(1);
    Ok((
        SdeIndex {
            path: path.to_path_buf(),
            id_index,
            name_index: HashMap::new(),
        },
        product_to_blueprint,
    ))
}

fn scan_stargates(path: &Path, pb: &ProgressBar) -> Result<HashMap<u64, Vec<u64>>> {
    pb.set_message(
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    );

    #[derive(serde::Deserialize)]
    struct Line {
        #[serde(rename = "systemID")]
        system_id: u64,
        destination: Destination,
    }
    #[derive(serde::Deserialize)]
    struct Destination {
        #[serde(rename = "systemID")]
        system_id: u64,
    }

    let file =
        fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::with_capacity(65536, file);
    let mut stargate_graph: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut buf = String::new();

    loop {
        buf.clear();
        let n = reader
            .read_line(&mut buf)
            .with_context(|| format!("read {}", path.display()))?;
        if n == 0 {
            break;
        }

        let trimmed = buf.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(parsed) = serde_json::from_str::<Line>(trimmed) {
            let src = parsed.system_id;
            let dst = parsed.destination.system_id;
            stargate_graph.entry(src).or_default().push(dst);
        }
    }

    pb.inc(1);
    Ok(stargate_graph)
}

fn extract_key(line: &[u8]) -> Option<u64> {
    let pos = memmem::find(line, b"\"_key\":")?;
    let rest = line[pos + 7..].trim_ascii_start();
    parse_u64_prefix(rest)
}

fn extract_name_en(line: &[u8]) -> Option<String> {
    let name_pos = memmem::find(line, b"\"name\":")?;
    let after_name = line[name_pos + 7..].trim_ascii_start();
    if after_name.first() != Some(&b'{') {
        return None;
    }
    let en_pos = memmem::find(after_name, b"\"en\":")?;
    let after_en = after_name[en_pos + 5..].trim_ascii_start();
    if after_en.first() != Some(&b'"') {
        return None;
    }
    decode_json_string(&after_en[1..])
}

fn parse_u64_prefix(s: &[u8]) -> Option<u64> {
    let end = s
        .iter()
        .position(|&b| !b.is_ascii_digit())
        .unwrap_or(s.len());
    if end == 0 {
        return None;
    }
    std::str::from_utf8(&s[..end]).ok()?.parse().ok()
}

fn decode_json_string(s: &[u8]) -> Option<String> {
    let mut out: Vec<u8> = Vec::with_capacity(64);
    let mut i = 0;
    while i < s.len() {
        match s[i] {
            b'"' => return String::from_utf8(out).ok(),
            b'\\' => {
                i += 1;
                if i >= s.len() {
                    return None;
                }
                match s[i] {
                    b'"' => out.push(b'"'),
                    b'\\' => out.push(b'\\'),
                    b'/' => out.push(b'/'),
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'b' => out.push(b'\x08'),
                    b'f' => out.push(b'\x0C'),
                    b'u' if i + 4 < s.len() => {
                        let hex = std::str::from_utf8(&s[i + 1..i + 5]).ok()?;
                        let code = u16::from_str_radix(hex, 16).ok()?;
                        if let Some(c) = char::from_u32(code as u32) {
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
                        }
                        i += 4;
                    }
                    _ => out.push(s[i]),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatif::ProgressBar;
    use std::io::Write;

    fn hidden_pb() -> ProgressBar {
        ProgressBar::hidden()
    }

    fn write_fixture(content: &str) -> (tempfile::NamedTempFile, PathBuf) {
        let mut f = tempfile::Builder::new()
            .suffix(".jsonl")
            .tempfile()
            .unwrap();
        f.write_all(content.as_bytes()).unwrap();
        let path = f.path().to_path_buf();
        (f, path)
    }

    #[test]
    fn extract_key_finds_underscore_key_field() {
        let line = br#"{"_key":34,"groupID":18,"name":{"en":"Tritanium"}}"#;
        assert_eq!(extract_key(line), Some(34));
    }

    #[test]
    fn extract_key_returns_none_when_missing() {
        let line = br#"{"id":34}"#;
        assert_eq!(extract_key(line), None);
    }

    #[test]
    fn extract_name_en_finds_english_name() {
        let line = br#"{"_key":34,"name":{"en":"Tritanium","de":"Tritanium"}}"#;
        assert_eq!(extract_name_en(line).as_deref(), Some("Tritanium"));
    }

    #[test]
    fn extract_name_en_returns_none_when_name_is_string_not_object() {
        let line = br#"{"_key":1,"name":"plain string"}"#;
        assert_eq!(extract_name_en(line), None);
    }

    #[test]
    fn extract_name_en_handles_escape_sequences() {
        let line = br#"{"_key":1,"name":{"en":"Ship\\Type"}}"#;
        assert_eq!(extract_name_en(line).as_deref(), Some("Ship\\Type"));
    }

    #[test]
    fn parse_u64_prefix_parses_number_before_delimiter() {
        assert_eq!(parse_u64_prefix(b"12345,rest"), Some(12345));
        assert_eq!(parse_u64_prefix(b"0}"), Some(0));
        assert_eq!(parse_u64_prefix(b"abc"), None);
    }

    #[test]
    fn scan_index_builds_id_and_name_indexes() {
        let fixture = r#"{"_key":34,"name":{"en":"Tritanium"}}
{"_key":35,"name":{"en":"Pyerite"}}
"#;
        let (_f, path) = write_fixture(fixture);
        let pb = hidden_pb();
        let idx = scan_index(&path, &pb).unwrap();

        assert_eq!(idx.id_index.len(), 2);
        assert!(idx.id_index.contains_key(&34));
        assert!(idx.id_index.contains_key(&35));
        assert!(idx.name_index.contains_key("tritanium"));
        assert!(idx.name_index.contains_key("pyerite"));
    }

    #[test]
    fn scan_index_offset_points_to_line_start() {
        let fixture = "{\"_key\":34,\"name\":{\"en\":\"Tritanium\"}}\n{\"_key\":35,\"name\":{\"en\":\"Pyerite\"}}\n";
        let (_f, path) = write_fixture(fixture);
        let pb = hidden_pb();
        let idx = scan_index(&path, &pb).unwrap();

        let off34 = *idx.id_index.get(&34).unwrap();
        let off35 = *idx.id_index.get(&35).unwrap();
        assert_eq!(off34, 0);
        assert!(off35 > off34);

        // Confirm offset 0 is the start of the first line
        let content = std::fs::read(&path).unwrap();
        let line_at_off34 = &content[off34 as usize..];
        assert!(line_at_off34.starts_with(b"{\"_key\":34"));
    }

    #[test]
    fn scan_blueprints_builds_product_to_blueprint_map() {
        let fixture = r#"{"_key":683,"activities":{"manufacturing":{"products":[{"typeID":582,"quantity":1}],"time":6000}}}
{"_key":684,"activities":{"copying":{"time":3600}}}
"#;
        let (_f, path) = write_fixture(fixture);
        let pb = hidden_pb();
        let (idx, p2b) = scan_blueprints(&path, &pb).unwrap();

        assert!(idx.id_index.contains_key(&683));
        assert!(idx.id_index.contains_key(&684));
        assert_eq!(p2b.get(&582), Some(&683));
        assert!(!p2b.contains_key(&684));
    }

    #[test]
    fn scan_stargates_builds_bidirectional_graph() {
        let fixture = r#"{"_key":50000056,"systemID":30000001,"destination":{"stargateID":50000055,"systemID":30000002}}
{"_key":50000055,"systemID":30000002,"destination":{"stargateID":50000056,"systemID":30000001}}
"#;
        let (_f, path) = write_fixture(fixture);
        let pb = hidden_pb();
        let graph = scan_stargates(&path, &pb).unwrap();

        let neighbors_1 = graph.get(&30000001).unwrap();
        let neighbors_2 = graph.get(&30000002).unwrap();
        assert!(neighbors_1.contains(&30000002));
        assert_eq!(neighbors_1.len(), 1, "no duplicate edges");
        assert!(neighbors_2.contains(&30000001));
        assert_eq!(neighbors_2.len(), 1, "no duplicate edges");
    }
}
