use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub(crate) struct SdeIndex {
    pub(crate) path: PathBuf,
    /// `_key` -> the byte offset of its line. The only map that addresses the file.
    pub(crate) id_index: HashMap<u64, u64>,
    /// Lowercased English name -> the `_key` of the record carrying it.
    ///
    /// A `_key` rather than a byte offset, even though a name search ends in a
    /// seek: an ID is what callers order by (ascending, so a query's answer is the
    /// same in every process, unlike `HashMap` iteration) and what every other
    /// index is keyed by, so a predicate like `published_only` or a Group scope
    /// filters the whole match set from memory instead of seeking and parsing a
    /// record per candidate. The offset is one `id_index` hit away.
    ///
    /// Only keyed lines are indexed here: a record with a name but no `_key` would
    /// be unreachable by name. No such record exists in build 3444265 — all 17
    /// scanned files key every line.
    pub(crate) name_index: HashMap<String, u64>,
}

/// Which blueprint activity yields a product. Manufacturing and reaction are the
/// two activities that have `products`; they are mutually exclusive per product
/// (the mfg-product and reaction-product sets are disjoint in the SDE), so a single
/// reverse map keyed by product can carry the activity tag unambiguously.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Activity {
    Manufacturing,
    Reaction,
}

impl Activity {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Activity::Manufacturing => "manufacturing",
            Activity::Reaction => "reaction",
        }
    }
}

/// A product's source blueprint plus the activity that produces it. Built into the
/// reverse index `product_to_blueprint` at scan time so callers can distinguish a
/// reaction output from a raw material (both previously looked like a bare `null`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct BlueprintRef {
    pub(crate) blueprint_id: u64,
    pub(crate) activity: Activity,
}

/// One entry from a dogma effect's `modifierInfo` array, flattened with the
/// effect it came from. Built into the reverse index `attribute_modifiers`
/// (keyed by `modified_attribute_id`) at scan time so "which skills/ships modify
/// attribute Y" is an O(1) lookup with no prose parsing.
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct ModifierRef {
    pub(crate) effect_id: u64,
    pub(crate) modifying_attribute_id: u64,
    pub(crate) modified_attribute_id: u64,
    pub(crate) operation: i64,
    pub(crate) func: Option<String>,
    pub(crate) domain: Option<String>,
    pub(crate) skill_type_id: Option<u64>,
}

/// One searchable DogmaAttribute or DogmaEffect: its ID and the English text
/// `sde_search_dogma` matches a substring against.
///
/// Held as text rather than as a byte offset — the one index here that is not an
/// offset — because the search spans three fields, and `SdeIndex::name_index` can
/// only key one exact string per record. It is also the only way these two files
/// are searchable at all: they store `name` as a bare string rather than the
/// `{"en": …}` map `extract_name_en` needs, so their `name_index` is empty.
///
/// English only. A localized field contributes its `en`, and the corpus stays at
/// the budgeted ~1 MB; carrying all eight SDE languages would multiply it by eight
/// to serve a step whose output is an ID.
#[derive(Debug)]
pub(crate) struct DogmaText {
    pub(crate) id: u32,
    /// The camelCase identifier (`jumpFatigueMultiplier`), which is why matching
    /// `name` alone is not enough: no spaced phrase occurs in one.
    pub(crate) name: Option<String>,
    /// The human label ("Jump Fatigue Multiplier"). Genuinely absent on some
    /// records — attributes 277/278 and effects 16/132/391 carry none — so a hit
    /// reports the fields it actually matched rather than assuming three exist.
    pub(crate) display_name: Option<String>,
    pub(crate) description: Option<String>,
    /// The DogmaAttribute's DefaultValue, and always `None` across the effect
    /// corpus: a DogmaEffect has no DefaultValue. It rides along here rather than
    /// justifying a scanner of its own because this corpus already holds a record
    /// per attribute — see ADR 0003's Consequences.
    pub(crate) default_value: Option<f64>,
}

pub(crate) struct SdeStore {
    pub(crate) data_dir: PathBuf,
    pub(crate) build: u64,
    pub(crate) release_date: String,
    pub(crate) files_scanned: usize,
    pub(crate) last_updated: String,
    pub(crate) types: SdeIndex,
    pub(crate) groups: SdeIndex,
    pub(crate) categories: SdeIndex,
    pub(crate) blueprints: SdeIndex,
    pub(crate) type_materials: SdeIndex,
    pub(crate) type_dogma: SdeIndex,
    pub(crate) map_solar_systems: SdeIndex,
    pub(crate) map_constellations: SdeIndex,
    pub(crate) map_regions: SdeIndex,
    pub(crate) npc_stations: SdeIndex,
    pub(crate) market_groups: SdeIndex,
    pub(crate) dogma_attributes: SdeIndex,
    pub(crate) dogma_effects: SdeIndex,
    pub(crate) factions: SdeIndex,
    pub(crate) npc_corporations: SdeIndex,
    pub(crate) skins: SdeIndex,
    pub(crate) product_to_blueprint: HashMap<u64, BlueprintRef>,
    pub(crate) stargate_graph: HashMap<u64, Vec<u64>>,
    /// modifiedAttributeID -> dogma modifiers that target it (reverse of dogmaEffects.modifierInfo)
    pub(crate) attribute_modifiers: HashMap<u64, Vec<ModifierRef>>,
    /// effectID -> type IDs whose dogmaEffects own this effect (reverse of typeDogma.dogmaEffects)
    pub(crate) effect_to_types: HashMap<u64, Vec<u64>>,
    /// attributeID -> the `(type_id, ExplicitValue)` pairs recorded against it
    /// (reverse of `typeDogma.dogmaAttributes`). Only ExplicitValues live here: a
    /// Type sitting at the DogmaAttribute's DefaultValue has no row in `typeDogma`
    /// and is therefore absent, which is the semantics `sde_find_types` reports.
    /// `u32`/`f32` rather than the `u64`/`f64` used elsewhere because this is the
    /// one index with ~646k entries in production — the narrow pair halves it to
    /// ~5 MB. Type and attribute IDs are far inside `u32`, and dogma values are
    /// `f32` in EVE's own engine.
    pub(crate) attribute_types: HashMap<u32, Vec<(u32, f32)>>,
    /// typeID -> its Group. Type↔Group is held in both directions because
    /// `sde_find_types` needs each one for a different job: this way for filtering
    /// and rolling up a candidate set that came from somewhere else, `group_types`
    /// for producing one. Extracted in the existing `types.jsonl` memmem pass, so
    /// a predicate over the full match set never costs a seek and parse per Type —
    /// a single DogmaAttribute can match 5,921 Types and a Category 11,836.
    pub(crate) type_group: HashMap<u32, u32>,
    /// typeID -> its MetaGroup, for the ~26% of Types that have one. Sparse on
    /// purpose and held in one direction only, unlike Type↔Group: a MetaGroup
    /// narrows a candidate set produced by another predicate, it does not produce
    /// one. Absence of a key is absence of a MetaGroup — never Tech I — which is
    /// what `sde_find_types` counts into `excluded_no_meta_group`. Also the source
    /// for `manufacturing::me_mode`, which used to seek and full-parse a Type
    /// record per node of a production chain to read the same field.
    pub(crate) type_meta_group: HashMap<u32, u32>,
    /// groupID -> the Types in it, ascending. The candidate set for a taxonomy-only
    /// `sde_find_types` call, sorted once at scan time exactly like
    /// `attribute_types`.
    pub(crate) group_types: HashMap<u32, Vec<u32>>,
    /// categoryID -> the Groups under it, ascending. A Category owns no Types
    /// directly; it resolves downward through its Groups, read from the already
    /// scanned `groups.jsonl`.
    pub(crate) category_groups: HashMap<u32, Vec<u32>>,
    /// The published Types, same pass. Membership rather than a per-Type flag, so
    /// a Type missing from `types.jsonl` is simply absent — `published_only` must
    /// admit only Types known to be published, never merely not-known-unpublished.
    /// Near enough half the SDE either way (26,983 of 52,821 in build 3444265), so
    /// there is no smaller side to store.
    pub(crate) published_types: HashSet<u32>,
    /// The DogmaAttribute text corpus `sde_search_dogma` scans, ascending by ID so
    /// a query's answer is ordered the same way in every process — `HashMap`
    /// iteration order is not stable across runs, and this list is walked whole.
    pub(crate) dogma_attribute_text: Vec<DogmaText>,
    /// The DogmaEffect half of the same corpus, so one call can answer a term the
    /// caller cannot yet classify as an attribute or an effect.
    pub(crate) dogma_effect_text: Vec<DogmaText>,
}
