//! Character progression: Skills, their Ranks, the SkillPoints curve, and the
//! SkillPlan that orders a set of prerequisites into something trainable.
//!
//! Reads Types and their dogma but owns neither — a Skill is a Type in the Skill
//! Category, and a prerequisite is a pair of DogmaAttributes. See CONTEXT.md.

use std::{collections::HashMap, sync::Arc};

use rmcp::{
    ErrorData,
    handler::server::wrapper::Parameters,
    schemars::{self, JsonSchema},
    tool, tool_router,
};
use serde::Deserialize;

use crate::store::SdeStore;
use crate::tools::SdeMcpServer;
use crate::tools::query;
use crate::tools::query::pick_name;

// ── Parameter structs ────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
pub struct SkillPlanTarget {
    /// Type ID to train prerequisites for (ship, module, or a skill itself)
    pub type_id: u64,
    /// When the target is a skill, train it to this level (default 5). Ignored for
    /// non-skill targets (their prerequisites keep the levels the item demands).
    pub level_override: Option<u8>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SkillPlanParam {
    /// Targets are treated as separate items (not a merged fit); no variant expansion
    pub targets: Vec<SkillPlanTarget>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SkillSpParam {
    /// Skill rank directly (skillTimeConstant, attribute 275)
    pub rank: Option<u64>,
    /// Or a skill's type ID — its rank is looked up from dogma
    pub type_id: Option<u64>,
}

#[tool_router(router = skills_router, vis = "pub(crate)")]
impl SdeMcpServer {
    #[tool(
        description = "Build a recursive skill-prerequisite training plan for one or more target type IDs (ships, modules, or skills). Returns each target's full prerequisite tree plus one merged, deduped (to the highest level demanded), topologically-sorted plan with per-skill rank, SP cost, running cumulative SP, the per-level SP curve (sp_by_level), and which targets require it."
    )]
    async fn sde_get_skill_plan(
        &self,
        Parameters(p): Parameters<SkillPlanParam>,
    ) -> Result<String, ErrorData> {
        let store = Arc::clone(&self.store);
        let lang = self.language.clone();
        let targets = p.targets;
        let plan = tokio::task::spawn_blocking(move || {
            build_skill_plan(&store, &targets, lang.as_deref())
        })
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
        .map_err(|e| ErrorData::invalid_params(e, None))?;
        Ok(serde_json::to_string(&plan).unwrap())
    }

    #[tool(
        description = "Get the SP cost curve (levels 1-5: cumulative sp_to_reach and per-level increment) for a skill. Provide rank directly, or type_id to look its rank up."
    )]
    async fn sde_get_skill_sp(
        &self,
        Parameters(p): Parameters<SkillSpParam>,
    ) -> Result<String, ErrorData> {
        let rank = match (p.rank, p.type_id) {
            (Some(rank), _) => rank,
            (None, Some(type_id)) => skill_rank(&self.store, type_id).ok_or_else(|| {
                ErrorData::invalid_params(
                    format!("type {type_id} is not a skill (no rank attribute 275)"),
                    None,
                )
            })?,
            (None, None) => {
                return Err(ErrorData::invalid_params("Provide rank or type_id", None));
            }
        };
        Ok(serde_json::to_string(&serde_json::json!({
            "rank": rank,
            "levels": sp_breakdown(rank),
        }))
        .unwrap())
    }
}

const ATTR_RANK: u64 = 275; // skillTimeConstant

const PREREQ_SLOTS: [(u64, u64); 3] = [(182, 277), (183, 278), (184, 279)]; // (skillID, levelID)

const MAX_SKILL_DEPTH: usize = 12;

/// Cumulative skill points to have a skill of the given rank at `level`.
/// EVE's canonical curve: SP(L) = round(rank · 250 · (√32)^(L-1)); √32 = 2^2.5.
/// Verified against the rank-1 points 250/1414/8000/45255/256000 — `round` (not ceil)
/// is what matches: 1414.21→1414, 45254.83→45255, and it absorbs the float noise that
/// makes the exact integer points (8000, 256000) compute as e.g. 256000.00000005.
fn skill_sp(rank: u64, level: u8) -> u64 {
    if level == 0 {
        return 0;
    }
    let sqrt32 = 32f64.sqrt();
    (rank as f64 * 250.0 * sqrt32.powi(level as i32 - 1)).round() as u64
}

/// A skill's own rank (attribute 275), or None if the type is not a skill.
fn skill_rank(store: &SdeStore, type_id: u64) -> Option<u64> {
    let dogma = query::fetch_by_id(&store.type_dogma, type_id).ok()?;
    let attrs = dogma.get("dogmaAttributes")?.as_array()?;
    attrs.iter().find_map(|a| {
        let aid = a.get("attributeID").and_then(|x| x.as_u64())?;
        (aid == ATTR_RANK)
            .then(|| a.get("value").and_then(|x| x.as_f64()))
            .flatten()
            .map(|v| v.round() as u64)
    })
}

/// A type's direct skill prerequisites as (skill_id, level) pairs.
fn direct_prereqs(store: &SdeStore, type_id: u64) -> Vec<(u64, u8)> {
    let Ok(dogma) = query::fetch_by_id(&store.type_dogma, type_id) else {
        return Vec::new();
    };
    let Some(attrs) = dogma.get("dogmaAttributes").and_then(|a| a.as_array()) else {
        return Vec::new();
    };
    let value_of = |attr_id: u64| -> Option<f64> {
        attrs.iter().find_map(|a| {
            let aid = a.get("attributeID").and_then(|x| x.as_u64())?;
            (aid == attr_id)
                .then(|| a.get("value").and_then(|x| x.as_f64()))
                .flatten()
        })
    };
    let mut out = Vec::new();
    for (skill_attr, level_attr) in PREREQ_SLOTS {
        if let Some(skill_id) = value_of(skill_attr) {
            let level = value_of(level_attr).unwrap_or(1.0).round().clamp(1.0, 5.0) as u8;
            out.push((skill_id as u64, level));
        }
    }
    out
}

/// serde `skip_serializing_if` predicate: omit a `bool` field when it's false.
fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(serde::Serialize, Debug)]
struct PrereqNode {
    skill_id: u64,
    skill_name: Option<String>,
    required_level: u8,
    rank: u64,
    /// True when `rank` was defaulted to 1 because the skill has no rank attribute
    /// (275) — its SP cost is therefore a lower-bound estimate, not authoritative.
    #[serde(skip_serializing_if = "is_false")]
    rank_assumed: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    prerequisites: Vec<PrereqNode>,
}

#[derive(serde::Serialize, Debug)]
struct SpLevel {
    level: u8,
    /// Total SP to have the skill at this level (cumulative from 0).
    sp_to_reach: u64,
    /// SP to train just this level, i.e. from level-1 to level.
    increment: u64,
}

/// Full SP cost curve (levels 1..=5) for a skill of the given rank.
fn sp_breakdown(rank: u64) -> Vec<SpLevel> {
    let mut prev = 0u64;
    (1..=5)
        .map(|level| {
            let sp_to_reach = skill_sp(rank, level);
            let increment = sp_to_reach - prev;
            prev = sp_to_reach;
            SpLevel {
                level,
                sp_to_reach,
                increment,
            }
        })
        .collect()
}

#[derive(serde::Serialize, Debug)]
struct PlanStep {
    skill_id: u64,
    skill_name: Option<String>,
    required_level: u8,
    rank: u64,
    /// See `PrereqNode::rank_assumed`.
    #[serde(skip_serializing_if = "is_false")]
    rank_assumed: bool,
    sp_for_level: u64,
    cumulative_sp: u64,
    /// Per-level SP cost (levels 1..=5) so callers can rank yield-per-SP without
    /// rebuilding the SP table by hand.
    sp_by_level: Vec<SpLevel>,
    required_by: Vec<u64>,
}

#[derive(serde::Serialize, Debug)]
struct TargetTree {
    type_id: u64,
    name: Option<String>,
    tree: Vec<PrereqNode>,
}

#[derive(serde::Serialize, Debug)]
struct SkillPlan {
    targets: Vec<TargetTree>,
    plan: Vec<PlanStep>,
    total_sp: u64,
}

#[derive(Default)]
struct Merged {
    level: u8,
    rank: u64,
    rank_assumed: bool,
    required_by: std::collections::BTreeSet<u64>,
}

/// Accumulators shared across one skill-plan call.
struct PlanAcc {
    merged: HashMap<u64, Merged>,
    edges: HashMap<u64, std::collections::BTreeSet<u64>>, // prereq_skill -> dependent skills
}

/// Recursively build the prereq tree for one skill, threading provenance and the
/// merged/edges accumulators. `path` is the active DFS stack for cycle detection.
fn build_node(
    store: &SdeStore,
    lang: Option<&str>,
    skill_id: u64,
    level: u8,
    target_id: u64,
    acc: &mut PlanAcc,
    path: &mut Vec<u64>,
) -> Result<PrereqNode, String> {
    if path.contains(&skill_id) {
        return Err(format!(
            "skill prerequisite cycle detected at skill {skill_id}"
        ));
    }
    if path.len() >= MAX_SKILL_DEPTH {
        return Err(format!(
            "skill prerequisite depth exceeds {MAX_SKILL_DEPTH}"
        ));
    }
    let (rank, rank_assumed) = match skill_rank(store, skill_id) {
        Some(r) => (r, false),
        None => {
            tracing::warn!(
                "skill-plan: skill {skill_id} has no rank attribute (275); \
                 assuming rank 1 — its SP cost is a lower-bound estimate"
            );
            (1, true)
        }
    };
    {
        let entry = acc.merged.entry(skill_id).or_default();
        entry.level = entry.level.max(level);
        entry.rank = rank;
        entry.rank_assumed = rank_assumed;
        entry.required_by.insert(target_id);
    }
    path.push(skill_id);
    let mut prerequisites = Vec::new();
    for (prereq_id, prereq_level) in direct_prereqs(store, skill_id) {
        acc.edges.entry(prereq_id).or_default().insert(skill_id);
        prerequisites.push(build_node(
            store,
            lang,
            prereq_id,
            prereq_level,
            target_id,
            acc,
            path,
        )?);
    }
    path.pop();
    Ok(PrereqNode {
        skill_id,
        skill_name: pick_name(
            query::fetch_by_id(&store.types, skill_id)
                .ok()
                .as_ref()
                .and_then(|v| v.get("name")),
            lang,
        ),
        required_level: level,
        rank,
        rank_assumed,
        prerequisites,
    })
}

fn build_skill_plan(
    store: &SdeStore,
    targets: &[SkillPlanTarget],
    lang: Option<&str>,
) -> Result<SkillPlan, String> {
    let mut acc = PlanAcc {
        merged: HashMap::new(),
        edges: HashMap::new(),
    };
    let mut target_trees = Vec::new();

    for target in targets {
        let mut path = Vec::new();
        let tree = if let Some(rank) = skill_rank(store, target.type_id) {
            // Target is itself a skill: train it (to override or 5) plus its prereqs.
            let _ = rank;
            let level = target.level_override.unwrap_or(5).clamp(1, 5);
            vec![build_node(
                store,
                lang,
                target.type_id,
                level,
                target.type_id,
                &mut acc,
                &mut path,
            )?]
        } else {
            // Ship/module: expand its direct skill prerequisites.
            let mut nodes = Vec::new();
            for (skill_id, level) in direct_prereqs(store, target.type_id) {
                nodes.push(build_node(
                    store,
                    lang,
                    skill_id,
                    level,
                    target.type_id,
                    &mut acc,
                    &mut path,
                )?);
            }
            nodes
        };
        target_trees.push(TargetTree {
            type_id: target.type_id,
            name: pick_name(
                query::fetch_by_id(&store.types, target.type_id)
                    .ok()
                    .as_ref()
                    .and_then(|v| v.get("name")),
                lang,
            ),
            tree,
        });
    }

    let order = topo_order(&acc)?;

    let mut plan = Vec::new();
    let mut cumulative = 0u64;
    for skill_id in order {
        let m = &acc.merged[&skill_id];
        let sp = skill_sp(m.rank, m.level);
        cumulative += sp;
        plan.push(PlanStep {
            skill_id,
            skill_name: pick_name(
                query::fetch_by_id(&store.types, skill_id)
                    .ok()
                    .as_ref()
                    .and_then(|v| v.get("name")),
                lang,
            ),
            required_level: m.level,
            rank: m.rank,
            rank_assumed: m.rank_assumed,
            sp_for_level: sp,
            cumulative_sp: cumulative,
            sp_by_level: sp_breakdown(m.rank),
            required_by: m.required_by.iter().copied().collect(),
        });
    }

    Ok(SkillPlan {
        targets: target_trees,
        plan,
        total_sp: cumulative,
    })
}

/// Kahn's algorithm over the merged skill set, popping lowest skill_id first for
/// stable output. Errors if a cycle leaves nodes unsorted.
fn topo_order(acc: &PlanAcc) -> Result<Vec<u64>, String> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    let mut indegree: HashMap<u64, usize> = acc.merged.keys().map(|&k| (k, 0)).collect();
    for deps in acc.edges.values() {
        for &s in deps {
            *indegree.entry(s).or_insert(0) += 1;
        }
    }
    let mut heap: BinaryHeap<Reverse<u64>> = indegree
        .iter()
        .filter(|&(_, &d)| d == 0)
        .map(|(&k, _)| Reverse(k))
        .collect();
    let mut order = Vec::with_capacity(acc.merged.len());
    while let Some(Reverse(n)) = heap.pop() {
        order.push(n);
        if let Some(deps) = acc.edges.get(&n) {
            for &s in deps {
                let d = indegree.get_mut(&s).unwrap();
                *d -= 1;
                if *d == 0 {
                    heap.push(Reverse(s));
                }
            }
        }
    }
    if order.len() != acc.merged.len() {
        return Err("skill prerequisite cycle detected".to_string());
    }
    Ok(order)
}

#[cfg(test)]
mod tests;
