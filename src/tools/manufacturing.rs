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

use serde_json::Value;

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

fn meta_group_of(store: &SdeStore, type_id: u64) -> Option<u64> {
    query::fetch_by_id(&store.types, type_id)
        .ok()?
        .get("metaGroupID")?
        .as_u64()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Activity, BlueprintRef, SdeIndex};
    use std::io::Write as _;

    fn write_fixture(content: &str) -> (tempfile::NamedTempFile, std::path::PathBuf) {
        let mut f = tempfile::Builder::new()
            .suffix(".jsonl")
            .tempfile()
            .unwrap();
        f.write_all(content.as_bytes()).unwrap();
        let path = f.path().to_path_buf();
        (f, path)
    }

    fn index(content: &str) -> (tempfile::NamedTempFile, SdeIndex) {
        let (f, path) = write_fixture(content);
        let pb = indicatif::ProgressBar::hidden();
        (f, crate::scan::scan_index_pub(&path, &pb).unwrap())
    }

    fn empty_index() -> SdeIndex {
        SdeIndex {
            path: std::path::PathBuf::from("/dev/null"),
            id_index: HashMap::new(),
            name_index: HashMap::new(),
        }
    }

    /// Minimal store wired with the indexes the classifier/engine read.
    struct Fixtures {
        _keep: Vec<tempfile::NamedTempFile>,
        store: SdeStore,
    }

    fn build_store(
        types_jsonl: &str,
        groups_jsonl: &str,
        blueprints_jsonl: &str,
        product_to_blueprint: HashMap<u64, BlueprintRef>,
    ) -> Fixtures {
        let (f1, types) = index(types_jsonl);
        let (f2, groups) = index(groups_jsonl);
        let (f3, blueprints) = index(blueprints_jsonl);
        let store = SdeStore {
            data_dir: std::path::PathBuf::from("/tmp"),
            build: 1,
            release_date: "2024-01-01".into(),
            files_scanned: 0,
            last_updated: "2024-01-01".into(),
            types,
            groups,
            categories: empty_index(),
            blueprints,
            type_materials: empty_index(),
            type_dogma: empty_index(),
            map_solar_systems: empty_index(),
            map_constellations: empty_index(),
            map_regions: empty_index(),
            npc_stations: empty_index(),
            market_groups: empty_index(),
            dogma_attributes: empty_index(),
            dogma_effects: empty_index(),
            factions: empty_index(),
            npc_corporations: empty_index(),
            skins: empty_index(),
            product_to_blueprint,
            stargate_graph: HashMap::new(),
            attribute_modifiers: HashMap::new(),
            effect_to_types: HashMap::new(),
        };
        Fixtures {
            _keep: vec![f1, f2, f3],
            store,
        }
    }

    fn p2b(entries: &[(u64, u64, Activity)]) -> HashMap<u64, BlueprintRef> {
        entries
            .iter()
            .map(|&(product, bp, activity)| {
                (
                    product,
                    BlueprintRef {
                        blueprint_id: bp,
                        activity,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn classify_origin_covers_every_variant() {
        // 100 manufactured (T1), 200 reaction output, 34 mineral, 16633 moon
        // material, 2393 PI commodity (category 43), 99 raw-other.
        let types = r#"{"_key":100,"name":{"en":"Widget"},"groupID":7,"metaGroupID":1}
{"_key":200,"name":{"en":"Polymer"},"groupID":429,"metaGroupID":null}
{"_key":34,"name":{"en":"Tritanium"},"groupID":18}
{"_key":16633,"name":{"en":"Hydrogen Isotopes"},"groupID":427}
{"_key":2393,"name":{"en":"Bacteria"},"groupID":1032}
{"_key":99,"name":{"en":"Mystery"},"groupID":555}
"#;
        let groups = r#"{"_key":7,"categoryID":6}
{"_key":429,"categoryID":24}
{"_key":18,"categoryID":4}
{"_key":427,"categoryID":4}
{"_key":1032,"categoryID":43}
{"_key":555,"categoryID":9}
"#;
        let bp = r#"{"_key":1100,"activities":{"manufacturing":{"products":[{"typeID":100,"quantity":1}]}}}
{"_key":1200,"activities":{"reaction":{"products":[{"typeID":200,"quantity":1}]}}}
"#;
        let map = p2b(&[
            (100, 1100, Activity::Manufacturing),
            (200, 1200, Activity::Reaction),
        ]);
        let fx = build_store(types, groups, bp, map);

        assert_eq!(classify_origin(&fx.store, 100), Origin::Manufactured);
        assert_eq!(classify_origin(&fx.store, 200), Origin::ReactionOutput);
        assert_eq!(classify_origin(&fx.store, 34), Origin::Mineral);
        assert_eq!(classify_origin(&fx.store, 16633), Origin::MoonMaterial);
        assert_eq!(classify_origin(&fx.store, 2393), Origin::PiOutput);
        assert_eq!(classify_origin(&fx.store, 99), Origin::RawOther);
    }

    #[test]
    fn me_mode_maps_meta_groups() {
        let types = r#"{"_key":1,"name":{"en":"T1"},"groupID":7,"metaGroupID":1}
{"_key":2,"name":{"en":"Faction"},"groupID":7,"metaGroupID":4}
{"_key":3,"name":{"en":"T2"},"groupID":7,"metaGroupID":2}
"#;
        let groups = r#"{"_key":7,"categoryID":6}
"#;
        let bp = r#"{"_key":11,"activities":{"manufacturing":{"products":[{"typeID":1,"quantity":1}]}}}
{"_key":12,"activities":{"manufacturing":{"products":[{"typeID":2,"quantity":1}]}}}
{"_key":13,"activities":{"manufacturing":{"products":[{"typeID":3,"quantity":1}]}}}
"#;
        let map = p2b(&[
            (1, 11, Activity::Manufacturing),
            (2, 12, Activity::Manufacturing),
            (3, 13, Activity::Manufacturing),
        ]);
        let fx = build_store(types, groups, bp, map);

        assert_eq!(me_mode(&fx.store, 1), Some(MeMode::Researchable));
        assert_eq!(me_mode(&fx.store, 2), Some(MeMode::FixedZero));
        assert_eq!(me_mode(&fx.store, 3), Some(MeMode::Invented));
        assert_eq!(me_mode(&fx.store, 34), None);
    }

    #[test]
    fn invented_target_is_not_buildable() {
        let types = r#"{"_key":3,"name":{"en":"T2 Module"},"groupID":7,"metaGroupID":2}
"#;
        let groups = r#"{"_key":7,"categoryID":7}
"#;
        let bp = r#"{"_key":13,"activities":{"manufacturing":{"products":[{"typeID":3,"quantity":1}]}}}
"#;
        let map = p2b(&[(3, 13, Activity::Manufacturing)]);
        let fx = build_store(types, groups, bp, map);

        let result = build_type(&fx.store, 3, None).unwrap();
        assert!(!result.buildable);
        assert!(result.reason.unwrap().contains("invention"));
    }

    /// A small two-tier chain: a manufactured widget (10/run) consumes 5 minerals +
    /// 3 of a reaction output per run; the reaction (200/run) consumes 100 of a moon
    /// material per run.
    fn small_chain() -> Fixtures {
        let types = r#"{"_key":100,"name":{"en":"Widget"},"groupID":7,"metaGroupID":1}
{"_key":200,"name":{"en":"Polymer"},"groupID":429,"metaGroupID":null}
{"_key":34,"name":{"en":"Tritanium"},"groupID":18}
{"_key":16633,"name":{"en":"Moon Goo"},"groupID":427}
{"_key":3380,"name":{"en":"Industry"},"groupID":150}
{"_key":45746,"name":{"en":"Reactions"},"groupID":150}
"#;
        let groups = r#"{"_key":7,"categoryID":6}
{"_key":429,"categoryID":24}
{"_key":18,"categoryID":4}
{"_key":427,"categoryID":4}
{"_key":150,"categoryID":16}
"#;
        let bp = r#"{"_key":1100,"activities":{"manufacturing":{"products":[{"typeID":100,"quantity":10}],"materials":[{"typeID":34,"quantity":5},{"typeID":200,"quantity":3}],"skills":[{"typeID":3380,"level":2}]}}}
{"_key":1200,"activities":{"reaction":{"products":[{"typeID":200,"quantity":200}],"materials":[{"typeID":16633,"quantity":100}],"skills":[{"typeID":45746,"level":3}]}}}
"#;
        let map = p2b(&[
            (100, 1100, Activity::Manufacturing),
            (200, 1200, Activity::Reaction),
        ]);
        build_store(types, groups, bp, map)
    }

    #[test]
    fn build_type_collects_origins_gates_and_skills() {
        let fx = small_chain();
        let bt = build_type(&fx.store, 100, None).unwrap();

        assert!(bt.buildable);
        assert_eq!(bt.me_mode, Some(MeMode::Researchable));
        assert_eq!(
            bt.decomposable_origins,
            vec![Origin::Manufactured, Origin::ReactionOutput]
        );
        // One gate for the reaction-output input (the target itself is not a gate).
        let reaction_gate = bt
            .gates
            .iter()
            .find(|g| g.build_origin == "reaction-output")
            .unwrap();
        assert_eq!(reaction_gate.inputs[0].type_id, 200);
        // Aggregate skills include both Industry and Reactions.
        let ids: Vec<u64> = bt.required_skills.iter().map(|s| s.skill_id).collect();
        assert!(ids.contains(&3380) && ids.contains(&45746));
    }

    #[test]
    fn production_chain_rounds_runs_and_applies_me() {
        let fx = small_chain();
        // Build everything, ME 0, 1 run of the widget (yields 10 units).
        let params = ChainParams {
            target_id: 100,
            runs: 1,
            build_origins: HashSet::from([Origin::Manufactured, Origin::ReactionOutput]),
            buy_type_ids: HashSet::new(),
            me_default: 0,
            me_overrides: HashMap::new(),
        };
        let chain = production_chain(&fx.store, &params, None).unwrap();

        // Widget: 1 run -> needs 5 Tritanium + 3 Polymer.
        let widget = chain.jobs.iter().find(|j| j.type_id == 100).unwrap();
        assert_eq!(widget.runs, 1);
        // Polymer demand 3 -> 1 reaction run (200/run), leftover 197.
        let polymer = chain.jobs.iter().find(|j| j.type_id == 200).unwrap();
        assert_eq!(polymer.runs, 1);
        assert_eq!(polymer.leftover, 197);
        assert_eq!(polymer.activity, "reaction");

        // Shopping list: 5 Tritanium (mineral) + 100 Moon Goo (moon material).
        let minerals = chain
            .shopping_list
            .iter()
            .find(|g| g.origin == Origin::Mineral)
            .unwrap();
        assert_eq!(minerals.items[0].type_id, 34);
        assert_eq!(minerals.items[0].quantity, 5);
        let moon = chain
            .shopping_list
            .iter()
            .find(|g| g.origin == Origin::MoonMaterial)
            .unwrap();
        assert_eq!(moon.items[0].quantity, 100);
    }

    #[test]
    fn production_chain_buys_when_origin_not_built() {
        let fx = small_chain();
        // Only build manufacturing; the reaction output becomes a buy leaf.
        let params = ChainParams {
            target_id: 100,
            runs: 1,
            build_origins: HashSet::from([Origin::Manufactured]),
            buy_type_ids: HashSet::new(),
            me_default: 0,
            me_overrides: HashMap::new(),
        };
        let chain = production_chain(&fx.store, &params, None).unwrap();

        // No reaction job; Polymer appears in the shopping list instead.
        assert!(chain.jobs.iter().all(|j| j.type_id != 200));
        let reaction_buy = chain
            .shopping_list
            .iter()
            .find(|g| g.origin == Origin::ReactionOutput)
            .unwrap();
        assert_eq!(reaction_buy.items[0].type_id, 200);
        assert_eq!(reaction_buy.items[0].quantity, 3);
    }

    #[test]
    fn production_chain_force_buys_override() {
        let fx = small_chain();
        let params = ChainParams {
            target_id: 100,
            runs: 1,
            build_origins: HashSet::from([Origin::Manufactured, Origin::ReactionOutput]),
            buy_type_ids: HashSet::from([200]),
            me_default: 0,
            me_overrides: HashMap::new(),
        };
        let chain = production_chain(&fx.store, &params, None).unwrap();
        // Override forces Polymer to a buy despite reaction-output being built.
        assert!(chain.jobs.iter().all(|j| j.type_id != 200));
        assert!(
            chain
                .shopping_list
                .iter()
                .any(|g| g.items.iter().any(|i| i.type_id == 200))
        );
    }

    /// Load the real SDE cache from the per-OS default data dir, or skip (return
    /// `None`) if it has not been downloaded. Run with `cargo test -- --ignored`.
    fn load_live_store() -> Option<std::sync::Arc<SdeStore>> {
        let dir = directories::ProjectDirs::from("", "", "eve-sde-mcp")?
            .data_dir()
            .to_path_buf();
        crate::scan::scan_sde(&dir, 0, "live").ok()
    }

    /// End-to-end acceptance against the live SDE: building a Nightmare (17736) with
    /// the canonical scenario — Nightmare BPC at ME 0, the three component BPOs at
    /// ME 10, fuel blocks bought — reproduces the consolidated shopping list and
    /// reaction job plan from `transcripts/nightmare-build.md:588-634`.
    #[test]
    #[ignore]
    fn nightmare_chain_matches_reference() {
        let Some(store) = load_live_store() else {
            eprintln!("live SDE cache not present — skipping");
            return;
        };
        let params = ChainParams {
            target_id: 17736,
            runs: 1,
            build_origins: HashSet::from([Origin::Manufactured, Origin::ReactionOutput]),
            buy_type_ids: HashSet::from([4051, 4246, 4247, 4312]), // fuel blocks
            me_default: 0,
            me_overrides: HashMap::from([(57479, 10), (57486, 10), (57478, 10)]),
        };
        let chain = production_chain(&store, &params, Some("en")).unwrap();

        let qty = |type_id: u64| -> u64 {
            chain
                .shopping_list
                .iter()
                .flat_map(|g| &g.items)
                .find(|i| i.type_id == type_id)
                .map(|i| i.quantity)
                .unwrap_or(0)
        };

        // Minerals (Nightmare BPC, ME 0).
        assert_eq!(qty(34), 9_600_000, "Tritanium");
        assert_eq!(qty(35), 4_800_000, "Pyerite");
        assert_eq!(qty(36), 720_000, "Mexallon");
        assert_eq!(qty(37), 480_000, "Isogen");
        assert_eq!(qty(38), 36_000, "Nocxium");
        assert_eq!(qty(39), 9_600, "Zydrine");
        assert_eq!(qty(40), 4_800, "Megacyte");

        // Fuel blocks (bought).
        assert_eq!(qty(4312), 50, "Oxygen Fuel Block");
        assert_eq!(qty(4246), 40, "Hydrogen Fuel Block");
        assert_eq!(qty(4247), 15, "Helium Fuel Block");
        assert_eq!(qty(4051), 15, "Nitrogen Fuel Block");

        // Sansha NET Resonator has no blueprint -> raw buy.
        assert_eq!(qty(83471), 160, "Sansha NET Resonator");

        // Composite reaction job plan: rounded runs and pre-round demand.
        let job = |type_id: u64| chain.jobs.iter().find(|j| j.type_id == type_id).unwrap();
        let rcf = job(57457); // Reinforced Carbon Fiber
        assert_eq!((rcf.runs, rcf.demand, rcf.total_output), (8, 1530, 1600));
        let pox = job(57456); // Pressurized Oxidizers
        assert_eq!((pox.runs, pox.demand, pox.total_output), (3, 450, 600));

        // Whole-chain job skills include Reactions and Industry.
        let skill_ids: Vec<u64> = chain.required_skills.iter().map(|s| s.skill_id).collect();
        assert!(skill_ids.contains(&3380), "Industry skill required");
        assert!(skill_ids.contains(&45746), "Reactions skill required");
    }

    #[test]
    fn material_with_me_floors_at_runs() {
        // 100 base, 10 runs, 10% ME -> 900, still above the 10-run floor.
        assert_eq!(material_with_me(100, 10, 10), 900);
        // 1 base, 5 runs, 90% ME -> ceil(0.5)=1 per... floor at runs=5.
        assert_eq!(material_with_me(1, 5, 90), 5);
    }
}
