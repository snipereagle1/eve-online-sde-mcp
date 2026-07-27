use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use memchr::memmem;
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::store::{Activity, BlueprintRef, DogmaText, ModifierRef, NameIndex, SdeIndex, SdeStore};

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

    let types = scan_types(&root.join("types.jsonl"), &pb)?;
    let (groups, category_groups) = scan_groups(&root.join("groups.jsonl"), &pb)?;
    let categories = scan_index(&root.join("categories.jsonl"), &pb)?;
    let (blueprints, product_to_blueprint) = scan_blueprints(&root.join("blueprints.jsonl"), &pb)?;
    let type_materials = scan_index(&root.join("typeMaterials.jsonl"), &pb)?;
    let (type_dogma, effect_to_types, attribute_types) =
        scan_type_dogma(&root.join("typeDogma.jsonl"), &pb)?;
    let map_solar_systems = scan_index(&root.join("mapSolarSystems.jsonl"), &pb)?;
    let map_constellations = scan_index(&root.join("mapConstellations.jsonl"), &pb)?;
    let map_regions = scan_index(&root.join("mapRegions.jsonl"), &pb)?;
    let stargate_graph = scan_stargates(&root.join("mapStargates.jsonl"), &pb)?;
    let npc_stations = scan_index(&root.join("npcStations.jsonl"), &pb)?;
    let market_groups = scan_index(&root.join("marketGroups.jsonl"), &pb)?;
    let (dogma_attributes, dogma_attribute_text) =
        scan_dogma_attributes(&root.join("dogmaAttributes.jsonl"), &pb)?;
    let (dogma_effects, attribute_modifiers, dogma_effect_text) =
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
        types: types.index,
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
        effect_to_types,
        attribute_types,
        type_group: types.type_group,
        group_types: types.group_types,
        type_meta_group: types.type_meta_group,
        category_groups,
        published_types: types.published_types,
        dogma_attribute_text,
        dogma_effect_text,
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

/// Exposed for `manufacturing`'s tests, whose stores have to carry the same
/// derived maps a real scan produces now that `me_mode` reads `type_meta_group`
/// instead of re-reading the Type record.
#[cfg(test)]
pub fn scan_types_pub(path: &Path, pb: &ProgressBar) -> Result<TypesScan> {
    scan_types(path, pb)
}

#[cfg(test)]
pub fn scan_blueprints_pub(
    path: &Path,
    pb: &ProgressBar,
) -> Result<(SdeIndex, HashMap<u64, BlueprintRef>)> {
    scan_blueprints(path, pb)
}

fn scan_index(path: &Path, pb: &ProgressBar) -> Result<SdeIndex> {
    scan_index_with(path, pb, |_, _| {})
}

/// The generic memmem pass, with a hook that sees every keyed line. Files that
/// need a derived map on top of `id`/`name` ride along here rather than reading
/// the file a second time — the whole point of the memmem approach is one pass.
/// `on_line` is called with the `_key` and the trimmed line bytes.
fn scan_index_with(
    path: &Path,
    pb: &ProgressBar,
    mut on_line: impl FnMut(u64, &[u8]),
) -> Result<SdeIndex> {
    pb.set_message(
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
    );

    let file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::with_capacity(65536, file);
    let mut id_index = HashMap::new();
    let mut name_index = NameIndex::default();
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
            on_line(key, trimmed);
            // Nested inside the key branch because `name_index` stores the `_key`,
            // not the offset — see `SdeIndex::name_index` for why.
            if let Some(name) = extract_name_en(trimmed) {
                name_index.insert(name.to_lowercase(), key);
            }
        }
    }

    // Sorted once here, like `group_types`, so a name shared by several records
    // has a stable internal order rather than the file's. Every scanned file
    // happens to be `_key`-ascending today; nothing guarantees the next one is.
    name_index.sort();

    pb.inc(1);
    Ok(SdeIndex {
        path: path.to_path_buf(),
        id_index,
        name_index,
    })
}

/// What one pass over types.jsonl yields. A struct rather than a tuple because
/// `type_group` and `type_meta_group` are both `HashMap<u32, u32>` and a caller
/// destructuring them the wrong way round would compile.
pub(crate) struct TypesScan {
    pub(crate) index: SdeIndex,
    pub(crate) type_group: HashMap<u32, u32>,
    pub(crate) group_types: HashMap<u32, Vec<u32>>,
    pub(crate) type_meta_group: HashMap<u32, u32>,
    pub(crate) published_types: HashSet<u32>,
}

/// Scan types.jsonl into the usual id/name indexes plus the Group taxonomy that
/// `sde_find_types` filters and rolls up on: `type_group`, its inverse
/// `group_types`, the MetaGroup of the Types that have one, and the set of
/// published Types.
///
/// `groupID`, `metaGroupID` and `published` ride along in this pass rather than
/// being seeked per Type at query time, because all three are needed for the
/// **whole** match set: `published_only` and the MetaGroup filter have to apply
/// before the limit, and the `groups` rollup has to count every match rather than
/// the returned page. Per-Type seeks would cost 11,836 reads for a single Category.
fn scan_types(path: &Path, pb: &ProgressBar) -> Result<TypesScan> {
    let mut type_group: HashMap<u32, u32> = HashMap::new();
    let mut group_types: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut type_meta_group: HashMap<u32, u32> = HashMap::new();
    let mut published_types: HashSet<u32> = HashSet::new();

    let index = scan_index_with(path, pb, |key, line| {
        // Types and Groups are keyed `u32` here to match `attribute_types`; a
        // `_key` beyond that range would be a schema change, and dropping it beats
        // truncating it onto another Type.
        let (Ok(type_id), Some(Ok(group_id))) = (
            u32::try_from(key),
            extract_number_field(line, b"\"groupID\":").map(u32::try_from),
        ) else {
            return;
        };
        type_group.insert(type_id, group_id);
        group_types.entry(group_id).or_default().push(type_id);
        // Sparse by design: 74% of Types have no `metaGroupID`, and the field is
        // also written as an explicit `null`, which parses as absent here. Either
        // way the Type stays out of the map, because a MetaGroup this scan invented
        // would be read as a tier the SDE never assigned.
        if let Some(Ok(meta_group_id)) =
            extract_number_field(line, b"\"metaGroupID\":").map(u32::try_from)
        {
            type_meta_group.insert(type_id, meta_group_id);
        }
        // Only a literal `true` enrolls a Type. Every Type in build 3444265
        // carries the field, so a missing one is unknown provenance and must not
        // slip past a `published_only` filter.
        if extract_bool_field(line, b"\"published\":") == Some(true) {
            published_types.insert(type_id);
        }
    })?;

    // Sorted once here, like `attribute_types`, so a taxonomy query inherits a
    // stable ascending type_id order instead of re-sorting per call.
    for types in group_types.values_mut() {
        types.sort_unstable();
    }

    Ok(TypesScan {
        index,
        type_group,
        group_types,
        type_meta_group,
        published_types,
    })
}

/// Scan groups.jsonl into the usual indexes plus `categoryID -> groups`. A
/// Category owns no Types directly, so `category_ids` resolves downward through
/// this map — the lookup the SDE's own records only express upward.
fn scan_groups(path: &Path, pb: &ProgressBar) -> Result<(SdeIndex, HashMap<u32, Vec<u32>>)> {
    let mut category_groups: HashMap<u32, Vec<u32>> = HashMap::new();

    let index = scan_index_with(path, pb, |key, line| {
        let (Ok(group_id), Some(Ok(category_id))) = (
            u32::try_from(key),
            extract_number_field(line, b"\"categoryID\":").map(u32::try_from),
        ) else {
            return;
        };
        category_groups
            .entry(category_id)
            .or_default()
            .push(group_id);
    })?;

    for groups in category_groups.values_mut() {
        groups.sort_unstable();
    }

    Ok((index, category_groups))
}

/// Scan dogmaAttributes.jsonl into the usual id/name indexes plus the attribute
/// half of the `sde_search_dogma` text corpus. Rides the existing memmem pass — no
/// second read of the file, and no new file scanned — but each line is also fully
/// parsed by the hook, which is affordable here and nowhere else: 2,141 records
/// against `types.jsonl`'s 52,821.
///
/// `defaultValue` is carried along with the text rather than seeked per query, per
/// ADR 0003: it costs nothing once a record per attribute is resident anyway.
fn scan_dogma_attributes(path: &Path, pb: &ProgressBar) -> Result<(SdeIndex, Vec<DogmaText>)> {
    /// The dogmaAttributes text fields. `name` and `description` are bare strings
    /// here while `displayName` is a localized map — the asymmetry
    /// [`en_text`] exists to absorb — so all three are read as untyped values.
    #[derive(serde::Deserialize)]
    struct AttributeText {
        name: Option<serde_json::Value>,
        #[serde(rename = "displayName")]
        display_name: Option<serde_json::Value>,
        description: Option<serde_json::Value>,
        #[serde(rename = "defaultValue")]
        default_value: Option<f64>,
    }

    let mut corpus: Vec<DogmaText> = Vec::new();
    let mut parse_failures = 0u64;
    let index = scan_index_with(path, pb, |key, line| {
        let parsed = match serde_json::from_slice::<AttributeText>(line) {
            Ok(parsed) => parsed,
            // Counted rather than swallowed, like the other custom scanners: a
            // schema drift here would empty the corpus and make `sde_search_dogma`
            // answer "no such attribute" for every query — the failure this whole
            // tool exists to end, arriving silently.
            Err(_) => {
                parse_failures += 1;
                return;
            }
        };
        let Ok(id) = u32::try_from(key) else { return };
        corpus.push(DogmaText {
            id,
            name: en_text(parsed.name.as_ref()),
            display_name: en_text(parsed.display_name.as_ref()),
            description: en_text(parsed.description.as_ref()),
            default_value: parsed.default_value,
        });
    })?;

    if parse_failures > 0 {
        tracing::warn!(
            "{}: {parse_failures} dogmaAttributes line(s) failed to parse; \
             sde_search_dogma cannot see them (possible SDE schema change)",
            path.display()
        );
    }

    corpus.sort_unstable_by_key(|record| record.id);
    Ok((index, corpus))
}

/// The English text of a dogma name, label or description, whichever of the two
/// shapes the SDE writes it in: a bare string (`dogmaAttributes.name` and
/// `.description`) or a localized map (`dogmaAttributes.displayName`,
/// `dogmaEffects.displayName` and `.description`). Taking either per field means
/// the corpus does not encode which file a record came from, and neither file's
/// shape is assumed — the assumption the empty `name_index` on these two files came
/// from in the first place.
///
/// An empty string is dropped rather than stored: it is not searchable text, and a
/// hit reporting it as the field that matched would be a lie.
fn en_text(value: Option<&serde_json::Value>) -> Option<String> {
    let text = match value? {
        serde_json::Value::String(s) => s.as_str(),
        serde_json::Value::Object(map) => map.get("en")?.as_str()?,
        _ => return None,
    };
    (!text.is_empty()).then(|| text.to_owned())
}

fn scan_blueprints(
    path: &Path,
    pb: &ProgressBar,
) -> Result<(SdeIndex, HashMap<u64, BlueprintRef>)> {
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
        manufacturing: Option<ActivityProducts>,
        reaction: Option<ActivityProducts>,
    }
    // Manufacturing and reaction share the `{products: [...]}` shape, so one struct
    // deserializes both arms.
    #[derive(serde::Deserialize)]
    struct ActivityProducts {
        products: Option<Vec<Product>>,
    }
    #[derive(serde::Deserialize)]
    struct Product {
        #[serde(rename = "typeID")]
        type_id: u64,
    }

    fn index_activity(
        map: &mut HashMap<u64, BlueprintRef>,
        activity: Option<ActivityProducts>,
        blueprint_id: u64,
        kind: Activity,
    ) {
        let Some(mut products) = activity.and_then(|a| a.products) else {
            return;
        };
        if products.is_empty() {
            return;
        }
        let product = products.swap_remove(0);
        map.entry(product.type_id)
            .and_modify(|existing| {
                if blueprint_id < existing.blueprint_id {
                    existing.blueprint_id = blueprint_id;
                    existing.activity = kind;
                }
            })
            .or_insert(BlueprintRef {
                blueprint_id,
                activity: kind,
            });
    }

    let file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
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
            if let Some(acts) = parsed.activities {
                // Each BP has exactly one product (verified), but a product can in
                // principle have several BPs; index the lowest blueprint ID
                // deterministically. Manufacturing and reaction product sets are
                // disjoint, so the activity tag is unambiguous.
                index_activity(
                    &mut product_to_blueprint,
                    acts.manufacturing,
                    parsed.key,
                    Activity::Manufacturing,
                );
                index_activity(
                    &mut product_to_blueprint,
                    acts.reaction,
                    parsed.key,
                    Activity::Reaction,
                );
            }
        }
    }

    pb.inc(1);
    Ok((
        SdeIndex {
            path: path.to_path_buf(),
            id_index,
            name_index: NameIndex::default(),
        },
        product_to_blueprint,
    ))
}

/// Scan typeDogma.jsonl into the id→offset index (like every other file) and two
/// reverse maps.
///
/// `effect_to_types` is keyed by `effectID`. A dogma effect's
/// `modifierInfo.skillTypeID` is only a required-skill *filter* on the boosted
/// modules, not the effect's source — the real source is the type whose
/// `dogmaEffects` array owns the effect. This reverse map records that ownership
/// so `sde_get_modifiers` direction-b can name the actual bonus source (e.g.
/// Astrogeology, not just Mining).
///
/// `attribute_types` is keyed by `attributeID` and holds every Type that records
/// an ExplicitValue for it. It rides along in this pass — which already
/// full-parses every line for `effect_to_types` — so `sde_find_types` never
/// touches the file at query time. Mirrors `scan_blueprints`'s tuple-returning,
/// typed-inner-struct pattern.
type TypeDogmaScan = (
    SdeIndex,
    HashMap<u64, Vec<u64>>,
    HashMap<u32, Vec<(u32, f32)>>,
);

fn scan_type_dogma(path: &Path, pb: &ProgressBar) -> Result<TypeDogmaScan> {
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
        #[serde(rename = "dogmaEffects")]
        dogma_effects: Option<Vec<EffectRef>>,
        #[serde(rename = "dogmaAttributes")]
        dogma_attributes: Option<Vec<AttributeRef>>,
    }
    #[derive(serde::Deserialize)]
    struct EffectRef {
        #[serde(rename = "effectID")]
        effect_id: u64,
    }
    #[derive(serde::Deserialize)]
    struct AttributeRef {
        #[serde(rename = "attributeID")]
        attribute_id: u32,
        value: Option<f32>,
    }

    let file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::with_capacity(65536, file);
    let mut id_index = HashMap::new();
    let mut effect_to_types: HashMap<u64, Vec<u64>> = HashMap::new();
    let mut attribute_types: HashMap<u32, Vec<(u32, f32)>> = HashMap::new();
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
            // Count rather than swallow: a schema drift would otherwise empty the
            // effect→owner map silently, making every modifier look source-less.
            Err(_) => {
                parse_failures += 1;
                continue;
            }
        };
        id_index.insert(parsed.key, line_start);
        for e in parsed.dogma_effects.into_iter().flatten() {
            effect_to_types
                .entry(e.effect_id)
                .or_default()
                .push(parsed.key);
        }
        // Types are keyed `u32` here; a `_key` beyond that range would be a schema
        // change, and dropping it is better than truncating it onto another Type.
        if let Ok(type_id) = u32::try_from(parsed.key) {
            for a in parsed.dogma_attributes.into_iter().flatten() {
                // A row with no `value` records no ExplicitValue, so it must not
                // become a phantom 0.0 that a `lt` predicate would match.
                let Some(value) = a.value else { continue };
                attribute_types
                    .entry(a.attribute_id)
                    .or_default()
                    .push((type_id, value));
            }
        }
    }

    if parse_failures > 0 {
        tracing::warn!(
            "{}: {parse_failures} typeDogma line(s) failed to parse; \
             effect_to_types reverse map may be incomplete (possible SDE schema change)",
            path.display()
        );
    }

    // Sorted once here so `sde_find_types` inherits a stable, ascending type_id
    // order for free on every query rather than re-sorting per call.
    for types in attribute_types.values_mut() {
        types.sort_unstable_by_key(|&(type_id, _)| type_id);
    }

    pb.inc(1);
    Ok((
        SdeIndex {
            path: path.to_path_buf(),
            id_index,
            name_index: NameIndex::default(),
        },
        effect_to_types,
        attribute_types,
    ))
}

/// Scan dogmaEffects.jsonl into the id→offset index (like every other file), a
/// reverse modifier map keyed by `modifiedAttributeID`, and the effect half of the
/// `sde_search_dogma` text corpus. Mirrors `scan_blueprints`'s tuple-returning,
/// typed-inner-struct pattern. `modifierInfo` ships as a real JSON array (verified
/// against build 3396210), so it deserializes straight into `Vec<RawMod>` with no
/// inner-string parsing.
///
/// The corpus rides this pass because it already full-parses every line: the text
/// costs three more fields on `Line`, not another read of the file.
type DogmaEffectsScan = (SdeIndex, HashMap<u64, Vec<ModifierRef>>, Vec<DogmaText>);

fn scan_dogma_effects(path: &Path, pb: &ProgressBar) -> Result<DogmaEffectsScan> {
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
        // Untyped for the same reason as in `scan_dogma_attributes`, and it is not
        // the same asymmetry: an effect's `name` is a bare string like an
        // attribute's, but its `description` is a localized map where the
        // attribute's is a bare string.
        name: Option<serde_json::Value>,
        #[serde(rename = "displayName")]
        display_name: Option<serde_json::Value>,
        description: Option<serde_json::Value>,
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
    let mut corpus: Vec<DogmaText> = Vec::new();
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
        if let Ok(id) = u32::try_from(parsed.key) {
            corpus.push(DogmaText {
                id,
                name: en_text(parsed.name.as_ref()),
                display_name: en_text(parsed.display_name.as_ref()),
                description: en_text(parsed.description.as_ref()),
                // A DogmaEffect has no DefaultValue; only the attribute half of the
                // corpus ever carries one.
                default_value: None,
            });
        }
        for m in parsed.modifier_info.into_iter().flatten() {
            // A modifier with no target attribute can't be reverse-indexed; skip it.
            let (Some(modified), Some(modifying)) = (m.modified, m.modifying) else {
                continue;
            };
            attribute_modifiers
                .entry(modified)
                .or_default()
                .push(ModifierRef {
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

    corpus.sort_unstable_by_key(|record| record.id);

    pb.inc(1);
    Ok((
        SdeIndex {
            path: path.to_path_buf(),
            id_index,
            name_index: NameIndex::default(),
        },
        attribute_modifiers,
        corpus,
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

    let file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
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
    extract_number_field(line, b"\"_key\":")
}

/// Read an unsigned integer field out of a raw JSONL line. `field` carries its own
/// opening quote (`"groupID":`), which is what keeps it from matching
/// `marketGroupID`; a JSON string can never contain an unescaped `"`, so the
/// needle cannot be found inside a localized name or description either.
fn extract_number_field(line: &[u8], field: &[u8]) -> Option<u64> {
    let pos = memmem::find(line, field)?;
    parse_u64_prefix(line[pos + field.len()..].trim_ascii_start())
}

/// As [`extract_number_field`], for a JSON boolean. Returns `None` when the field
/// is absent or holds something other than `true`/`false`, leaving the caller to
/// decide what absence means.
fn extract_bool_field(line: &[u8], field: &[u8]) -> Option<bool> {
    let pos = memmem::find(line, field)?;
    let rest = line[pos + field.len()..].trim_ascii_start();
    if rest.starts_with(b"true") {
        Some(true)
    } else if rest.starts_with(b"false") {
        Some(false)
    } else {
        None
    }
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
        let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sde");
        let store = scan_sde(&fixture_dir, crate::sde_version::PINNED_BUILD, "2024-01-15").unwrap();

        assert_eq!(store.build, crate::sde_version::PINNED_BUILD);
        assert_eq!(store.files_scanned, 17);

        assert!(store.types.id_index.contains_key(&34), "Tritanium missing");
        assert!(
            !store.types.name_index.ids_for("tritanium").is_empty(),
            "Tritanium name index missing"
        );
        assert!(store.types.id_index.contains_key(&16227), "Ferox missing");

        assert!(
            store.map_solar_systems.id_index.contains_key(&30000142),
            "Jita missing"
        );
        assert!(
            store.map_solar_systems.id_index.contains_key(&30000144),
            "Perimeter missing"
        );
        assert!(
            !store
                .map_solar_systems
                .name_index
                .ids_for("jita")
                .is_empty(),
            "Jita name index missing"
        );

        assert!(
            store.blueprints.id_index.contains_key(&16228),
            "Ferox Blueprint missing"
        );
        let ferox_bp = store
            .product_to_blueprint
            .get(&16227)
            .expect("Ferox product->blueprint map missing");
        assert_eq!(ferox_bp.blueprint_id, 16228);
        assert_eq!(ferox_bp.activity, Activity::Manufacturing);

        assert!(
            store.market_groups.id_index.contains_key(&1857),
            "Minerals market group missing"
        );
        assert!(
            store.factions.id_index.contains_key(&500001),
            "Caldari State faction missing"
        );
        assert!(
            store.npc_corporations.id_index.contains_key(&1000035),
            "Caldari Navy corp missing"
        );
        assert!(store.skins.id_index.contains_key(&50), "Ferox skin missing");
        assert!(
            store.dogma_attributes.id_index.contains_key(&263),
            "shieldCapacity attr missing"
        );
        assert!(
            store.type_dogma.id_index.contains_key(&16227),
            "Ferox typeDogma missing"
        );

        let jita_neighbors = store
            .stargate_graph
            .get(&30000142)
            .expect("Jita has no stargate neighbors");
        assert!(
            jita_neighbors.contains(&30000144),
            "Jita->Perimeter stargate missing"
        );
    }

    #[test]
    fn fixture_fetch_by_id_and_search_by_name() {
        use crate::tools::query;
        let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sde");
        let store = scan_sde(&fixture_dir, crate::sde_version::PINNED_BUILD, "2024-01-15").unwrap();

        let tritanium = query::fetch_by_id(&store.types, 34).unwrap();
        assert_eq!(tritanium["_key"], 34);
        assert_eq!(tritanium["groupID"], 18);

        let results = query::search_by_name(&store.types, "ferox", 10, |_| true).unwrap();
        let keys: Vec<_> = results.iter().filter_map(|v| v["_key"].as_u64()).collect();
        assert!(
            keys.contains(&16227),
            "search 'ferox' should find Ferox type"
        );

        let jita = query::fetch_by_id(&store.map_solar_systems, 30000142).unwrap();
        assert_eq!(jita["_key"], 30000142);
        assert_eq!(jita["securityStatus"].as_f64().unwrap(), 0.945913);

        let ferox_bp = query::fetch_by_id(&store.blueprints, 16228).unwrap();
        assert_eq!(ferox_bp["_key"], 16228);
        let products = ferox_bp["activities"]["manufacturing"]["products"]
            .as_array()
            .unwrap();
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
        assert!(!idx.name_index.ids_for("tritanium").is_empty());
        assert!(!idx.name_index.ids_for("pyerite").is_empty());
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
        let r = p2b.get(&582).unwrap();
        assert_eq!(r.blueprint_id, 683);
        assert_eq!(r.activity, Activity::Manufacturing);
        assert!(!p2b.contains_key(&684));
    }

    #[test]
    fn scan_blueprints_tags_reaction_products() {
        let fixture = r#"{"_key":57493,"activities":{"reaction":{"products":[{"typeID":57457,"quantity":200}],"time":3600}}}
"#;
        let (_f, path) = write_fixture(fixture);
        let pb = hidden_pb();
        let (_idx, p2b) = scan_blueprints(&path, &pb).unwrap();

        let r = p2b.get(&57457).unwrap();
        assert_eq!(r.blueprint_id, 57493);
        assert_eq!(r.activity, Activity::Reaction);
    }

    #[test]
    fn scan_blueprints_keeps_lowest_blueprint_id_per_product() {
        // Two manufacturing BPs yield the same product; the lower BP ID wins.
        let fixture = r#"{"_key":900,"activities":{"manufacturing":{"products":[{"typeID":582,"quantity":1}]}}}
{"_key":800,"activities":{"manufacturing":{"products":[{"typeID":582,"quantity":1}]}}}
"#;
        let (_f, path) = write_fixture(fixture);
        let pb = hidden_pb();
        let (_idx, p2b) = scan_blueprints(&path, &pb).unwrap();

        assert_eq!(p2b.get(&582).unwrap().blueprint_id, 800);
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
        let (idx, mods, _) = scan_dogma_effects(&path, &pb).unwrap();

        assert!(idx.id_index.contains_key(&391));
        assert!(
            idx.id_index.contains_key(&11),
            "effect with no usable modifier still indexed by id"
        );

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
    fn scan_type_dogma_builds_effect_to_types_reverse_map() {
        // Effect 391 is owned by BOTH Mining (3386) and Astrogeology (3410) — each
        // type's dogmaEffects lists it. The reverse map must surface both owners, so
        // sde_get_modifiers can name Astrogeology as a yield source, not just Mining.
        let fixture = r#"{"_key":3386,"dogmaAttributes":[{"attributeID":434,"value":5.0}],"dogmaEffects":[{"effectID":391,"isDefault":false}]}
{"_key":3410,"dogmaAttributes":[{"attributeID":434,"value":5.0}],"dogmaEffects":[{"effectID":391,"isDefault":false}]}
{"_key":34,"dogmaAttributes":[],"dogmaEffects":[]}
"#;
        let (_f, path) = write_fixture(fixture);
        let pb = hidden_pb();
        let (idx, eff_to_types, _) = scan_type_dogma(&path, &pb).unwrap();

        assert!(idx.id_index.contains_key(&3386));
        assert!(idx.id_index.contains_key(&3410));
        assert!(
            idx.id_index.contains_key(&34),
            "type with no effects still indexed by id"
        );

        let owners = eff_to_types.get(&391).expect("effect 391 has owning types");
        assert_eq!(owners.len(), 2);
        assert!(owners.contains(&3386), "Mining owns effect 391");
        assert!(owners.contains(&3410), "Astrogeology owns effect 391");
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
