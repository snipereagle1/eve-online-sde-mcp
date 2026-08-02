//! Manufacturing planning: classify a Type's production origin, route a build
//! decision tree (`build_type`), and compute a quantity-resolved production chain
//! (`production_chain`).
//!
//! These live in their own module rather than `server.rs` because the engine is a
//! self-contained graph algorithm (classify → DFS the build DAG → topo-accumulate
//! demand → run-round with material efficiency). The `#[tool]` entry points in
//! `server.rs` are thin `spawn_blocking` wrappers over the free functions here,
//! mirroring how `sde_get_skill_plan` wraps `build_skill_plan`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;

use rmcp::{
    ErrorData,
    handler::server::wrapper::Parameters,
    schemars::{self, JsonSchema},
    tool, tool_router,
};
use serde::Deserialize;

use super::SdeMcpServer;
use super::query;
use crate::store::{Activity, SdeStore};

// ── Classification constants ─────────────────────────────────────────────────

/// Group/category IDs used to classify raw (non-blueprint) leaves. Verified
/// against SDE build 3400955.
const GROUP_MINERAL: u64 = 18;
const GROUP_MOON_MATERIAL: u64 = 427;
const CATEGORY_PI: u64 = 43; // Planetary Commodities

/// Hard ceiling on build-tree recursion depth — a corrupt or cyclic SDE would
/// otherwise blow the stack. Real chains are <10 deep.
const MAX_DEPTH: usize = 32;

// ── Origin / MeMode ──────────────────────────────────────────────────────────

/// How a Type comes into existence. Blueprint-first: a product with a blueprint is
/// `Manufactured` or `ReactionOutput`; everything else is a leaf classified by
/// group/category. The kebab-case serialization doubles as the `build_origins`
/// toggle vocabulary on `sde_get_production_chain`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Origin {
    Manufactured,
    ReactionOutput,
    Mineral,
    MoonMaterial,
    PiOutput,
    RawOther,
}

impl Origin {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Origin::Manufactured => "manufactured",
            Origin::ReactionOutput => "reaction-output",
            Origin::Mineral => "mineral",
            Origin::MoonMaterial => "moon-material",
            Origin::PiOutput => "pi-output",
            Origin::RawOther => "raw-other",
        }
    }

    /// Parse a `build_origins` toggle string. Only the two decomposable origins are
    /// meaningful toggles, but any valid key parses for forward-compatibility.
    pub(crate) fn from_key(s: &str) -> Option<Origin> {
        match s {
            "manufactured" => Some(Origin::Manufactured),
            "reaction-output" => Some(Origin::ReactionOutput),
            "mineral" => Some(Origin::Mineral),
            "moon-material" => Some(Origin::MoonMaterial),
            "pi-output" => Some(Origin::PiOutput),
            "raw-other" => Some(Origin::RawOther),
            _ => None,
        }
    }

    /// Human-readable group label for a shopping-list section.
    fn label(self) -> &'static str {
        match self {
            Origin::Manufactured => "Manufactured (bought)",
            Origin::ReactionOutput => "Reaction outputs (bought)",
            Origin::Mineral => "Minerals",
            Origin::MoonMaterial => "Moon Materials",
            Origin::PiOutput => "Planetary (PI)",
            Origin::RawOther => "Other (buy/loot)",
        }
    }
}

/// Material-efficiency regime of a `Manufactured` product, from its `metaGroupID`.
/// Determines whether ME can be researched (`Researchable`), is locked at 0
/// (`FixedZero`, e.g. faction/officer), or requires invention (`Invented`,
/// out of scope — treated as a buy leaf and flagged).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum MeMode {
    Researchable,
    FixedZero,
    Invented,
}

impl MeMode {
    fn from_meta_group(meta_group_id: Option<u64>) -> MeMode {
        match meta_group_id {
            None | Some(1) => MeMode::Researchable,
            Some(2) | Some(14) => MeMode::Invented,
            Some(3) | Some(4) | Some(5) | Some(6) => MeMode::FixedZero,
            // Unknown meta groups: assume a normal researchable T1-like item.
            Some(_) => MeMode::Researchable,
        }
    }
}

// ── Type-record helpers ──────────────────────────────────────────────────────

fn group_of(store: &SdeStore, type_id: u64) -> Option<u64> {
    query::fetch_by_id(&store.types, type_id)
        .ok()?
        .get("groupID")?
        .as_u64()
}

fn category_of(store: &SdeStore, type_id: u64) -> Option<u64> {
    let group_id = group_of(store, type_id)?;
    query::fetch_by_id(&store.groups, group_id)
        .ok()?
        .get("categoryID")?
        .as_u64()
}

/// The Type's MetaGroup, or `None` when it has none — which is most Types, and
/// which [`MeMode::from_meta_group`] reads as "no invention or faction lock".
///
/// A scan-time map hit rather than the seek + read + full JSON parse this used to
/// do. `me_mode` runs once per node of a production chain, so the parse was being
/// paid per Type on the deepest call the server has.
fn meta_group_of(store: &SdeStore, type_id: u64) -> Option<u64> {
    let type_id = u32::try_from(type_id).ok()?;
    store.type_meta_group.get(&type_id).copied().map(u64::from)
}

fn type_name(store: &SdeStore, type_id: u64, lang: Option<&str>) -> Option<String> {
    let value = query::fetch_by_id(&store.types, type_id).ok()?;
    match value.get("name")? {
        Value::String(s) => Some(s.clone()),
        Value::Object(map) => lang
            .and_then(|l| map.get(l))
            .or_else(|| map.get("en"))
            .and_then(|v| v.as_str())
            .map(String::from),
        _ => None,
    }
}

/// Classify how `type_id` is produced. Blueprint-first, falling back to a
/// group/category lookup for raw leaves.
pub(crate) fn classify_origin(store: &SdeStore, type_id: u64) -> Origin {
    if let Some(bp_ref) = store.product_to_blueprint.get(&type_id) {
        return match bp_ref.activity {
            Activity::Manufacturing => Origin::Manufactured,
            Activity::Reaction => Origin::ReactionOutput,
        };
    }
    match group_of(store, type_id) {
        Some(GROUP_MINERAL) => Origin::Mineral,
        Some(GROUP_MOON_MATERIAL) => Origin::MoonMaterial,
        _ => {
            if category_of(store, type_id) == Some(CATEGORY_PI) {
                Origin::PiOutput
            } else {
                Origin::RawOther
            }
        }
    }
}

/// ME mode of a manufactured product, or `None` if it is not manufactured.
pub(crate) fn me_mode(store: &SdeStore, type_id: u64) -> Option<MeMode> {
    if classify_origin(store, type_id) != Origin::Manufactured {
        return None;
    }
    Some(MeMode::from_meta_group(meta_group_of(store, type_id)))
}

/// A Type is decomposable (worth building further) when it has a blueprint and is
/// not an invented item (invention math is out of scope, so invented items are
/// terminal buy leaves).
pub(crate) fn is_decomposable(store: &SdeStore, type_id: u64) -> bool {
    match classify_origin(store, type_id) {
        Origin::ReactionOutput => true,
        Origin::Manufactured => me_mode(store, type_id) != Some(MeMode::Invented),
        _ => false,
    }
}

// ── Blueprint formula ────────────────────────────────────────────────────────

/// The producing blueprint's recipe for one product, flattened for the engine.
struct Formula {
    activity: Activity,
    /// Units of the product yielded per run.
    output_per_run: u64,
    /// `(material_type_id, quantity_per_run)`.
    materials: Vec<(u64, u64)>,
    /// `(skill_type_id, level)` required to run the job.
    skills: Vec<(u64, u8)>,
}

/// Resolve the producing blueprint recipe for `type_id`, or `None` if it has no
/// blueprint (a raw leaf).
fn formula_for(store: &SdeStore, type_id: u64) -> Option<Formula> {
    let bp_ref = store.product_to_blueprint.get(&type_id)?;
    let blueprint = query::fetch_by_id(&store.blueprints, bp_ref.blueprint_id).ok()?;
    let activity_obj = blueprint.get("activities")?.get(bp_ref.activity.as_str())?;

    let output_per_run = activity_obj
        .get("products")
        .and_then(|p| p.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|p| p.get("typeID").and_then(Value::as_u64) == Some(type_id))
                .or_else(|| arr.first())
        })
        .and_then(|p| p.get("quantity").and_then(Value::as_u64))
        .unwrap_or(1)
        .max(1);

    let materials = activity_obj
        .get("materials")
        .and_then(|m| m.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| Some((m.get("typeID")?.as_u64()?, m.get("quantity")?.as_u64()?)))
                .collect()
        })
        .unwrap_or_default();

    let skills = activity_obj
        .get("skills")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|s| Some((s.get("typeID")?.as_u64()?, s.get("level")?.as_u64()? as u8)))
                .collect()
        })
        .unwrap_or_default();

    Some(Formula {
        activity: bp_ref.activity,
        output_per_run,
        materials,
        skills,
    })
}

// ── Router: build_type ───────────────────────────────────────────────────────

#[derive(serde::Serialize, Debug)]
pub(crate) struct SkillReq {
    pub(crate) skill_id: u64,
    pub(crate) skill_name: Option<String>,
    pub(crate) level: u8,
}

#[derive(serde::Serialize, Debug)]
pub(crate) struct GateInput {
    pub(crate) type_id: u64,
    pub(crate) name: Option<String>,
    pub(crate) origin: Origin,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) me_mode: Option<MeMode>,
    pub(crate) required_skills: Vec<SkillReq>,
}

/// One buy-vs-build toggle: enabling `build_origin` builds every `inputs` entry of
/// this origin; leaving it off buys them as-is.
#[derive(serde::Serialize, Debug)]
pub(crate) struct DecisionGate {
    pub(crate) build_origin: &'static str,
    pub(crate) inputs: Vec<GateInput>,
}

#[derive(serde::Serialize, Debug)]
pub(crate) struct FlaggedLeaf {
    pub(crate) type_id: u64,
    pub(crate) name: Option<String>,
    pub(crate) origin: Origin,
    pub(crate) reason: &'static str,
}

#[derive(serde::Serialize, Debug)]
pub(crate) struct BuildType {
    pub(crate) type_id: u64,
    pub(crate) name: Option<String>,
    pub(crate) buildable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) me_mode: Option<MeMode>,
    /// Distinct decomposable origins present anywhere in the build tree — the set of
    /// toggles meaningful for `sde_get_production_chain`.
    pub(crate) decomposable_origins: Vec<Origin>,
    /// Per-origin buy-vs-build decision gates.
    pub(crate) gates: Vec<DecisionGate>,
    /// All blueprint-job skills needed across the chain, deduped to the highest
    /// level demanded.
    pub(crate) required_skills: Vec<SkillReq>,
    /// Leaves that cannot be built within scope (invention / PI) — surfaced so the
    /// caller knows why a branch stops at a buy.
    pub(crate) flagged_leaves: Vec<FlaggedLeaf>,
}

#[derive(Default)]
struct RouterAcc {
    decomposable_origins: BTreeSet<Origin>,
    /// origin -> (type_id -> GateInput) so each input is recorded once.
    gates: BTreeMap<Origin, BTreeMap<u64, GateInput>>,
    /// skill_id -> highest level demanded.
    skills: BTreeMap<u64, u8>,
    /// type_id -> flagged leaf (deduped).
    flagged: BTreeMap<u64, FlaggedLeaf>,
    visited: HashSet<u64>,
}

fn merge_skills(acc: &mut BTreeMap<u64, u8>, skills: &[(u64, u8)]) {
    for &(skill_id, level) in skills {
        let entry = acc.entry(skill_id).or_insert(0);
        *entry = (*entry).max(level);
    }
}

fn router_dfs(
    store: &SdeStore,
    type_id: u64,
    depth: usize,
    is_target: bool,
    lang: Option<&str>,
    path: &mut Vec<u64>,
    acc: &mut RouterAcc,
) -> Result<(), String> {
    if path.contains(&type_id) {
        return Err(format!("production cycle detected at type {type_id}"));
    }
    if depth > MAX_DEPTH {
        return Err(format!("production tree depth exceeds {MAX_DEPTH}"));
    }
    if !acc.visited.insert(type_id) {
        return Ok(()); // already expanded via another parent (shared intermediate)
    }

    let origin = classify_origin(store, type_id);

    if is_decomposable(store, type_id) {
        let formula = match formula_for(store, type_id) {
            Some(f) => f,
            None => return Ok(()),
        };
        acc.decomposable_origins.insert(origin);
        merge_skills(&mut acc.skills, &formula.skills);

        if !is_target {
            let required_skills = formula
                .skills
                .iter()
                .map(|&(skill_id, level)| SkillReq {
                    skill_id,
                    skill_name: type_name(store, skill_id, lang),
                    level,
                })
                .collect();
            acc.gates
                .entry(origin)
                .or_default()
                .entry(type_id)
                .or_insert(GateInput {
                    type_id,
                    name: type_name(store, type_id, lang),
                    origin,
                    me_mode: me_mode(store, type_id),
                    required_skills,
                });
        }

        path.push(type_id);
        for (mat_id, _qty) in &formula.materials {
            router_dfs(store, *mat_id, depth + 1, false, lang, path, acc)?;
        }
        path.pop();
    } else if !is_target {
        // Terminal leaf. Flag the out-of-scope ones so the caller sees why the
        // branch stops at a buy rather than continuing to decompose.
        let reason = match origin {
            Origin::Manufactured => Some("invention required (out of scope)"),
            Origin::PiOutput => Some("planetary industry (out of scope)"),
            _ => None,
        };
        if let Some(reason) = reason {
            acc.flagged.entry(type_id).or_insert(FlaggedLeaf {
                type_id,
                name: type_name(store, type_id, lang),
                origin,
                reason,
            });
        }
    }

    Ok(())
}

/// Classify-only router over the full build tree. Reports what is buildable, the
/// decomposable origins present, per-origin buy-vs-build gates, the aggregate job
/// skills, and out-of-scope leaves. No quantity math — that is
/// `production_chain`'s job.
pub(crate) fn build_type(
    store: &SdeStore,
    target_id: u64,
    lang: Option<&str>,
) -> Result<BuildType, String> {
    let name = type_name(store, target_id, lang);

    if !is_decomposable(store, target_id) {
        let origin = classify_origin(store, target_id);
        let reason = match origin {
            Origin::Manufactured => {
                "target requires invention (out of scope); buy it or its components"
            }
            _ => "target has no manufacturing or reaction blueprint — it is a raw item",
        };
        return Ok(BuildType {
            type_id: target_id,
            name,
            buildable: false,
            reason: Some(reason.to_string()),
            me_mode: None,
            decomposable_origins: Vec::new(),
            gates: Vec::new(),
            required_skills: Vec::new(),
            flagged_leaves: Vec::new(),
        });
    }

    let mut acc = RouterAcc::default();
    let mut path = Vec::new();
    router_dfs(store, target_id, 0, true, lang, &mut path, &mut acc)?;

    let gates = acc
        .gates
        .into_iter()
        .map(|(origin, inputs)| DecisionGate {
            build_origin: origin.key(),
            inputs: inputs.into_values().collect(),
        })
        .collect();

    let required_skills = acc
        .skills
        .into_iter()
        .map(|(skill_id, level)| SkillReq {
            skill_id,
            skill_name: type_name(store, skill_id, lang),
            level,
        })
        .collect();

    Ok(BuildType {
        type_id: target_id,
        name,
        buildable: true,
        reason: None,
        me_mode: me_mode(store, target_id),
        decomposable_origins: acc.decomposable_origins.into_iter().collect(),
        gates,
        required_skills,
        flagged_leaves: acc.flagged.into_values().collect(),
    })
}

// ── Engine: production_chain ─────────────────────────────────────────────────

/// Player decisions that drive the quantity engine. Built from the
/// `sde_get_production_chain` tool parameters.
pub(crate) struct ChainParams {
    pub(crate) target_id: u64,
    pub(crate) runs: u64,
    /// Decomposable origins to build (others are bought as leaves).
    pub(crate) build_origins: HashSet<Origin>,
    /// Force-buy these type IDs even if their origin is being built.
    pub(crate) buy_type_ids: HashSet<u64>,
    pub(crate) me_default: i64,
    pub(crate) me_overrides: HashMap<u64, i64>,
}

impl ChainParams {
    /// Material efficiency to apply to a manufacturing job for `type_id`, clamped to
    /// a sane 0..=100 range.
    fn me_for(&self, type_id: u64) -> i64 {
        self.me_overrides
            .get(&type_id)
            .copied()
            .unwrap_or(self.me_default)
            .clamp(0, 100)
    }

    /// Whether `type_id` should be built (vs bought). The target is always built;
    /// other nodes build only when decomposable, their origin is toggled on, and
    /// they are not force-bought.
    fn builds(&self, store: &SdeStore, type_id: u64, is_target: bool) -> bool {
        if self.buy_type_ids.contains(&type_id) && !is_target {
            return false;
        }
        if !is_decomposable(store, type_id) {
            return false;
        }
        if is_target {
            return true;
        }
        self.build_origins
            .contains(&classify_origin(store, type_id))
    }
}

#[derive(serde::Serialize, Debug)]
pub(crate) struct Job {
    pub(crate) type_id: u64,
    pub(crate) name: Option<String>,
    pub(crate) activity: &'static str,
    pub(crate) runs: u64,
    pub(crate) output_per_run: u64,
    pub(crate) total_output: u64,
    pub(crate) demand: u64,
    pub(crate) leftover: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) me_applied: Option<i64>,
}

#[derive(serde::Serialize, Debug)]
pub(crate) struct ShopItem {
    pub(crate) type_id: u64,
    pub(crate) name: Option<String>,
    pub(crate) quantity: u64,
}

#[derive(serde::Serialize, Debug)]
pub(crate) struct ShopGroup {
    pub(crate) origin: Origin,
    pub(crate) label: &'static str,
    pub(crate) items: Vec<ShopItem>,
}

#[derive(serde::Serialize, Debug)]
pub(crate) struct ChainTarget {
    pub(crate) type_id: u64,
    pub(crate) name: Option<String>,
    pub(crate) runs: u64,
}

#[derive(serde::Serialize, Debug)]
pub(crate) struct ProductionChain {
    pub(crate) target: ChainTarget,
    /// Build jobs ordered top-down (target first, then its sub-builds).
    pub(crate) jobs: Vec<Job>,
    /// Consolidated buy list grouped by origin.
    pub(crate) shopping_list: Vec<ShopGroup>,
    /// Aggregate job skills across the whole chain (deduped to highest level).
    pub(crate) required_skills: Vec<SkillReq>,
    pub(crate) flagged_leaves: Vec<FlaggedLeaf>,
}

fn ceil_div(numerator: u64, denominator: u64) -> u64 {
    if denominator == 0 {
        return numerator;
    }
    numerator.div_ceil(denominator)
}

/// Material quantity for a manufacturing job: ME reduces per-unit cost, but a job
/// always consumes at least one unit per run. Reactions ignore ME (call with the
/// flat path instead).
fn material_with_me(base_per_run: u64, runs: u64, me: i64) -> u64 {
    let me = me.clamp(0, 100) as u64;
    let reduced = ceil_div(base_per_run * runs * (100 - me), 100);
    reduced.max(runs)
}

/// Recursively discover every node in the build DAG, recording formulas for built
/// nodes and edges from each built node to its materials.
#[allow(clippy::too_many_arguments)]
fn discover(
    store: &SdeStore,
    params: &ChainParams,
    type_id: u64,
    depth: usize,
    is_target: bool,
    path: &mut Vec<u64>,
    formulas: &mut HashMap<u64, Formula>,
    leaves: &mut HashSet<u64>,
) -> Result<(), String> {
    if path.contains(&type_id) {
        return Err(format!("production cycle detected at type {type_id}"));
    }
    if depth > MAX_DEPTH {
        return Err(format!("production tree depth exceeds {MAX_DEPTH}"));
    }
    if formulas.contains_key(&type_id) || leaves.contains(&type_id) {
        return Ok(()); // already discovered via another parent
    }

    if params.builds(store, type_id, is_target) {
        let formula = match formula_for(store, type_id) {
            Some(f) => f,
            None => {
                leaves.insert(type_id);
                return Ok(());
            }
        };
        let materials = formula.materials.clone();
        formulas.insert(type_id, formula);
        path.push(type_id);
        for (mat_id, _qty) in &materials {
            discover(
                store,
                params,
                *mat_id,
                depth + 1,
                false,
                path,
                formulas,
                leaves,
            )?;
        }
        path.pop();
    } else {
        leaves.insert(type_id);
    }
    Ok(())
}

/// Kahn topological order over the built nodes (parents before children) so a
/// shared intermediate's demand is fully accumulated before its runs are rounded.
fn topo_built(formulas: &HashMap<u64, Formula>) -> Result<Vec<u64>, String> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    let mut indegree: HashMap<u64, usize> = formulas.keys().map(|&k| (k, 0)).collect();
    for formula in formulas.values() {
        for (mat_id, _) in &formula.materials {
            if let Some(d) = indegree.get_mut(mat_id) {
                *d += 1;
            }
        }
    }
    let mut heap: BinaryHeap<Reverse<u64>> = indegree
        .iter()
        .filter(|&(_, &d)| d == 0)
        .map(|(&k, _)| Reverse(k))
        .collect();
    let mut order = Vec::with_capacity(formulas.len());
    while let Some(Reverse(node)) = heap.pop() {
        order.push(node);
        if let Some(formula) = formulas.get(&node) {
            for (mat_id, _) in &formula.materials {
                if let Some(d) = indegree.get_mut(mat_id) {
                    *d -= 1;
                    if *d == 0 {
                        heap.push(Reverse(*mat_id));
                    }
                }
            }
        }
    }
    if order.len() != formulas.len() {
        return Err("production cycle detected among built nodes".to_string());
    }
    Ok(order)
}

/// Quantity engine: given player decisions, resolve the full chain — per-Type build
/// jobs (runs, leftover) and one consolidated shopping list grouped by origin.
pub(crate) fn production_chain(
    store: &SdeStore,
    params: &ChainParams,
    lang: Option<&str>,
) -> Result<ProductionChain, String> {
    if !is_decomposable(store, params.target_id) {
        return Err("target has no manufacturing or reaction blueprint".to_string());
    }
    let runs = params.runs.max(1);

    let mut formulas: HashMap<u64, Formula> = HashMap::new();
    let mut leaves: HashSet<u64> = HashSet::new();
    let mut path = Vec::new();
    discover(
        store,
        params,
        params.target_id,
        0,
        true,
        &mut path,
        &mut formulas,
        &mut leaves,
    )?;

    let order = topo_built(&formulas)?;

    // Demand in product units, accumulated parent-first.
    let mut demand: HashMap<u64, u64> = HashMap::new();
    let target_output = formulas
        .get(&params.target_id)
        .map(|f| f.output_per_run)
        .unwrap_or(1);
    demand.insert(params.target_id, runs * target_output);

    let mut skills: BTreeMap<u64, u8> = BTreeMap::new();
    let mut jobs = Vec::new();

    for type_id in &order {
        let formula = &formulas[type_id];
        let demanded = demand.get(type_id).copied().unwrap_or(0);
        let job_runs = ceil_div(demanded, formula.output_per_run);
        merge_skills(&mut skills, &formula.skills);

        let me_applied = match formula.activity {
            Activity::Manufacturing => Some(params.me_for(*type_id)),
            Activity::Reaction => None,
        };

        for &(mat_id, base_per_run) in &formula.materials {
            let qty = match formula.activity {
                Activity::Reaction => base_per_run * job_runs,
                Activity::Manufacturing => {
                    material_with_me(base_per_run, job_runs, params.me_for(*type_id))
                }
            };
            *demand.entry(mat_id).or_insert(0) += qty;
        }

        let total_output = job_runs * formula.output_per_run;
        jobs.push(Job {
            type_id: *type_id,
            name: type_name(store, *type_id, lang),
            activity: formula.activity.as_str(),
            runs: job_runs,
            output_per_run: formula.output_per_run,
            total_output,
            demand: demanded,
            leftover: total_output - demanded,
            me_applied,
        });
    }

    // Shopping list: leaf demand grouped by origin.
    let mut grouped: BTreeMap<Origin, Vec<ShopItem>> = BTreeMap::new();
    let mut flagged: BTreeMap<u64, FlaggedLeaf> = BTreeMap::new();
    for leaf in &leaves {
        let qty = demand.get(leaf).copied().unwrap_or(0);
        if qty == 0 {
            continue;
        }
        let origin = classify_origin(store, *leaf);
        grouped.entry(origin).or_default().push(ShopItem {
            type_id: *leaf,
            name: type_name(store, *leaf, lang),
            quantity: qty,
        });
        let reason = match origin {
            Origin::Manufactured if me_mode(store, *leaf) == Some(MeMode::Invented) => {
                Some("invention required (out of scope)")
            }
            Origin::PiOutput => Some("planetary industry (out of scope)"),
            _ => None,
        };
        if let Some(reason) = reason {
            flagged.entry(*leaf).or_insert(FlaggedLeaf {
                type_id: *leaf,
                name: type_name(store, *leaf, lang),
                origin,
                reason,
            });
        }
    }

    let shopping_list = grouped
        .into_iter()
        .map(|(origin, mut items)| {
            items.sort_by(|a, b| b.quantity.cmp(&a.quantity).then(a.type_id.cmp(&b.type_id)));
            ShopGroup {
                origin,
                label: origin.label(),
                items,
            }
        })
        .collect();

    let required_skills = skills
        .into_iter()
        .map(|(skill_id, level)| SkillReq {
            skill_id,
            skill_name: type_name(store, skill_id, lang),
            level,
        })
        .collect();

    Ok(ProductionChain {
        target: ChainTarget {
            type_id: params.target_id,
            name: type_name(store, params.target_id, lang),
            runs,
        },
        jobs,
        shopping_list,
        required_skills,
        flagged_leaves: flagged.into_values().collect(),
    })
}

// ── Parameter structs ────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
pub struct BuildTypeParam {
    /// The Type ID you want to manufacture/build (a ship, module, component, etc.)
    pub product_type_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct ProductionChainParam {
    /// The Type ID you want to build.
    pub product_type_id: u64,
    /// Number of runs (units, when output-per-run is 1) of the target to build. Default 1.
    pub runs: Option<u64>,
    /// Which decomposable origins to build rather than buy: any of "manufactured",
    /// "reaction-output". Defaults to both (build the whole tree). Anything not built
    /// lands in the shopping list.
    pub build_origins: Option<Vec<String>>,
    /// Force these Type IDs to be bought even when their origin is being built
    /// (e.g. buy fuel blocks instead of decomposing them into ice + PI).
    pub buy_type_ids: Option<Vec<u64>>,
    /// Default material efficiency (%) applied to every manufacturing job. Default 0.
    /// Reactions always ignore ME.
    pub me: Option<i64>,
    /// Per-Type material-efficiency overrides (%), keyed by Type ID; overrides `me`
    /// for those types only.
    pub me_overrides: Option<HashMap<u64, i64>>,
}

// ── Tool implementations ─────────────────────────────────────────────────────

#[tool_router(router = manufacturing_router, vis = "pub(crate)")]
impl SdeMcpServer {
    #[tool(
        description = "Plan how to manufacture / build / produce a Type (ship, module, component, …): the FIRST tool to call for 'how do I build X', 'what do I need to make X', 'bill of materials', or 'production chain'. Classifies the whole build tree and returns: whether the target is buildable (and its material-efficiency mode), the distinct decomposable origins present (manufactured vs reaction-output), per-origin buy-vs-build decision gates (each input tagged with its origin, ME mode, and required skills), the aggregate blueprint-job skills across the chain, and any out-of-scope leaves (invention or planetary-industry items you must buy). This is the classify-only router — neutral facts, no recommendations. Once the player picks what to build vs buy, call sde_get_production_chain for the resolved quantities and shopping list."
    )]
    async fn sde_build_type(
        &self,
        Parameters(p): Parameters<BuildTypeParam>,
    ) -> Result<String, ErrorData> {
        let store = Arc::clone(&self.store);
        let lang = self.language.clone();
        let target_id = p.product_type_id;
        let result =
            tokio::task::spawn_blocking(move || build_type(&store, target_id, lang.as_deref()))
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
                .map_err(|e| ErrorData::invalid_params(e, None))?;
        Ok(serde_json::to_string(&result).unwrap())
    }

    #[tool(
        description = "Compute the resolved production chain for a build: given the player's buy-vs-build decisions, returns per-Type build jobs (runs, output-per-run, leftover from run-rounding) and one consolidated shopping list grouped by origin (minerals, moon materials, PI, etc.), plus the aggregate job skills. Material efficiency reduces manufacturing material cost (floored at one unit per run); reactions ignore ME. Shared intermediates are counted once across the whole tree before run-rounding. Decisions: build_origins toggles which decomposable origins to build (default: build everything), buy_type_ids force-buys specific Types (e.g. fuel blocks), me / me_overrides set material efficiency. Call sde_build_type first to discover the decision gates."
    )]
    async fn sde_get_production_chain(
        &self,
        Parameters(p): Parameters<ProductionChainParam>,
    ) -> Result<String, ErrorData> {
        let build_origins: HashSet<Origin> = match p.build_origins {
            Some(keys) => {
                let mut set = HashSet::new();
                for key in keys {
                    let origin = Origin::from_key(&key).ok_or_else(|| {
                        ErrorData::invalid_params(
                            format!(
                                "unknown build_origin '{key}' (expected 'manufactured' or 'reaction-output')"
                            ),
                            None,
                        )
                    })?;
                    set.insert(origin);
                }
                set
            }
            None => HashSet::from([Origin::Manufactured, Origin::ReactionOutput]),
        };

        let params = ChainParams {
            target_id: p.product_type_id,
            runs: p.runs.unwrap_or(1),
            build_origins,
            buy_type_ids: p.buy_type_ids.unwrap_or_default().into_iter().collect(),
            me_default: p.me.unwrap_or(0),
            me_overrides: p.me_overrides.unwrap_or_default(),
        };

        let store = Arc::clone(&self.store);
        let lang = self.language.clone();
        let result =
            tokio::task::spawn_blocking(move || production_chain(&store, &params, lang.as_deref()))
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
                .map_err(|e| ErrorData::invalid_params(e, None))?;
        Ok(serde_json::to_string(&result).unwrap())
    }
}

#[cfg(test)]
mod tests;
