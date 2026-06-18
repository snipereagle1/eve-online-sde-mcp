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

use crate::store::{ModifierRef, SdeIndex, SdeStore};

const SDE_FILE_COUNT: u64 = 17;

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
    let type_dogma = scan_index(&root.join("typeDogma.jsonl"), &pb)?;
    let map_solar_systems = scan_index(&root.join("mapSolarSystems.jsonl"), &pb)?;
    let map_constellations = scan_index(&root.join("mapConstellations.jsonl"), &pb)?;
    let map_regions = scan_index(&root.join("mapRegions.jsonl"), &pb)?;
    let stargate_graph = scan_stargates(&root.join("mapStargates.jsonl"), &pb)?;
    let npc_stations = scan_index(&root.join("npcStations.jsonl"), &pb)?;
    let market_groups = scan_index(&root.join("marketGroups.jsonl"), &pb)?;
    let dogma_attributes = scan_index(&root.join("dogmaAttributes.jsonl"), &pb)?;
    let (dogma_effects, attribute_modifiers) =
        scan_dogma_effects(&root.join("dogmaEffects.jsonl"), &pb)?;
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
        type_dogma,
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
        attribute_modifiers,
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

/// Scan dogmaEffects.jsonl into both the id→offset index (like every other file)
/// and a reverse modifier map keyed by `modifiedAttributeID`. Mirrors
/// `scan_blueprints`'s tuple-returning, typed-inner-struct pattern. `modifierInfo`
/// ships as a real JSON array (verified against build 3396210), so it deserializes
/// straight into `Vec<RawMod>` with no inner-string parsing.
fn scan_dogma_effects(
    path: &Path,
    pb: &ProgressBar,
) -> Result<(SdeIndex, HashMap<u64, Vec<ModifierRef>>)> {
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
        #[serde(rename = "modifierInfo")]
        modifier_info: Option<Vec<RawMod>>,
    }
    #[derive(serde::Deserialize)]
    struct RawMod {
        domain: Option<String>,
        func: Option<String>,
        #[serde(rename = "modifiedAttributeID")]
        modified: Option<u64>,
        #[serde(rename = "modifyingAttributeID")]
        modifying: Option<u64>,
        operation: Option<i64>,
        #[serde(rename = "skillTypeID")]
        skill_type_id: Option<u64>,
    }

    let file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::with_capacity(65536, file);
    let mut id_index = HashMap::new();
    let mut attribute_modifiers: HashMap<u64, Vec<ModifierRef>> = HashMap::new();
    let mut buf = String::new();
    let mut offset = 0u64;
    let mut parse_failures = 0u64;

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
        let parsed = match serde_json::from_str::<Line>(trimmed) {
            Ok(parsed) => parsed,
            // Count rather than swallow: a schema drift (e.g. modifierInfo shape
            // changing) would otherwise empty the reverse map silently, making
            // `sde_get_modifiers` look like "no modifiers exist". Surface it.
            Err(_) => {
                parse_failures += 1;
                continue;
            }
        };
        id_index.insert(parsed.key, line_start);
        for m in parsed.modifier_info.into_iter().flatten() {
            // A modifier with no target attribute can't be reverse-indexed; skip it.
            let (Some(modified), Some(modifying)) = (m.modified, m.modifying) else {
                continue;
            };
            attribute_modifiers.entry(modified).or_default().push(ModifierRef {
                effect_id: parsed.key,
                modifying_attribute_id: modifying,
                modified_attribute_id: modified,
                operation: m.operation.unwrap_or(0),
                func: m.func,
                domain: m.domain,
                skill_type_id: m.skill_type_id,
            });
        }
    }

    if parse_failures > 0 {
        tracing::warn!(
            "{}: {parse_failures} dogmaEffects line(s) failed to parse; \
             attribute_modifiers reverse map may be incomplete (possible SDE schema change)",
            path.display()
        );
    }

    pb.inc(1);
    Ok((
        SdeIndex {
            path: path.to_path_buf(),
            id_index,
            name_index: HashMap::new(),
        },
        attribute_modifiers,
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
        #[serde(rename = "solarSystemID")]
        system_id: u64,
        destination: Destination,
    }
    #[derive(serde::Deserialize)]
    struct Destination {
        #[serde(rename = "solarSystemID")]
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

    #[test]
    fn scan_sde_fixture_dir_indexes_all_17_files() {
        let fixture_dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sde");
        let store =
            scan_sde(&fixture_dir, crate::sde_version::PINNED_BUILD, "2024-01-15").unwrap();

        assert_eq!(store.build, crate::sde_version::PINNED_BUILD);
        assert_eq!(store.files_scanned, 17);

        assert!(store.types.id_index.contains_key(&34), "Tritanium missing");
        assert!(store.types.name_index.contains_key("tritanium"), "Tritanium name index missing");
        assert!(store.types.id_index.contains_key(&16227), "Ferox missing");

        assert!(store.map_solar_systems.id_index.contains_key(&30000142), "Jita missing");
        assert!(store.map_solar_systems.id_index.contains_key(&30000144), "Perimeter missing");
        assert!(store.map_solar_systems.name_index.contains_key("jita"), "Jita name index missing");

        assert!(store.blueprints.id_index.contains_key(&16228), "Ferox Blueprint missing");
        assert_eq!(
            store.product_to_blueprint.get(&16227),
            Some(&16228),
            "Ferox product->blueprint map missing"
        );

        assert!(store.market_groups.id_index.contains_key(&1857), "Minerals market group missing");
        assert!(store.factions.id_index.contains_key(&500001), "Caldari State faction missing");
        assert!(store.npc_corporations.id_index.contains_key(&1000035), "Caldari Navy corp missing");
        assert!(store.skins.id_index.contains_key(&50), "Ferox skin missing");
        assert!(store.dogma_attributes.id_index.contains_key(&263), "shieldCapacity attr missing");
        assert!(store.type_dogma.id_index.contains_key(&16227), "Ferox typeDogma missing");

        let jita_neighbors = store.stargate_graph.get(&30000142).expect("Jita has no stargate neighbors");
        assert!(jita_neighbors.contains(&30000144), "Jita->Perimeter stargate missing");
    }

    #[test]
    fn fixture_fetch_by_id_and_search_by_name() {
        use crate::tools::query;
        let fixture_dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sde");
        let store =
            scan_sde(&fixture_dir, crate::sde_version::PINNED_BUILD, "2024-01-15").unwrap();

        let tritanium = query::fetch_by_id(&store.types, 34).unwrap();
        assert_eq!(tritanium["_key"], 34);
        assert_eq!(tritanium["groupID"], 18);

        let results = query::search_by_name(&store.types, "ferox", 10).unwrap();
        let keys: Vec<_> = results.iter().filter_map(|v| v["_key"].as_u64()).collect();
        assert!(keys.contains(&16227), "search 'ferox' should find Ferox type");

        let jita = query::fetch_by_id(&store.map_solar_systems, 30000142).unwrap();
        assert_eq!(jita["_key"], 30000142);
        assert_eq!(jita["securityStatus"].as_f64().unwrap(), 0.945913);

        let ferox_bp = query::fetch_by_id(&store.blueprints, 16228).unwrap();
        assert_eq!(ferox_bp["_key"], 16228);
        let products = ferox_bp["activities"]["manufacturing"]["products"].as_array().unwrap();
        assert_eq!(products[0]["typeID"], 16227);
    }

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
    fn scan_dogma_effects_builds_reverse_modifier_map() {
        // Effect 391: Astrogeology (skill 3386) applies miningAmountBonus (434) to
        // miningAmount (77). A second entry with no modifiedAttributeID must be skipped.
        let fixture = r#"{"_key":391,"name":{"en":"miningBonus"},"modifierInfo":[{"domain":"shipID","func":"LocationRequiredSkillModifier","modifiedAttributeID":77,"modifyingAttributeID":434,"operation":6,"skillTypeID":3386}]}
{"_key":11,"name":{"en":"loPower"},"modifierInfo":[{"func":"ItemModifier","modifyingAttributeID":50,"operation":2}]}
"#;
        let (_f, path) = write_fixture(fixture);
        let pb = hidden_pb();
        let (idx, mods) = scan_dogma_effects(&path, &pb).unwrap();

        assert!(idx.id_index.contains_key(&391));
        assert!(idx.id_index.contains_key(&11), "effect with no usable modifier still indexed by id");

        let to_mining = mods.get(&77).expect("attribute 77 has modifiers");
        assert_eq!(to_mining.len(), 1);
        let m = &to_mining[0];
        assert_eq!(m.effect_id, 391);
        assert_eq!(m.modifying_attribute_id, 434);
        assert_eq!(m.skill_type_id, Some(3386));
        assert_eq!(m.operation, 6);
        // The modifier missing modifiedAttributeID was skipped, not indexed.
        assert!(mods.values().flatten().all(|m| m.effect_id != 11));
    }

    #[test]
    fn scan_stargates_builds_bidirectional_graph() {
        let fixture = r#"{"_key":50000056,"solarSystemID":30000001,"destination":{"stargateID":50000055,"solarSystemID":30000002}}
{"_key":50000055,"solarSystemID":30000002,"destination":{"stargateID":50000056,"solarSystemID":30000001}}
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
