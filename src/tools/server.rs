use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use rmcp::{
    ErrorData, ServerHandler,
    handler::server::wrapper::Parameters,
    model::{Implementation, ServerCapabilities, ServerInfo},
    schemars::{self, JsonSchema},
    tool, tool_handler, tool_router,
};
use serde::Deserialize;

use super::manufacturing;
use super::query;
use crate::store::SdeStore;

// ── Parameter structs ────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
pub struct TypeIdParam {
    /// EVE type ID
    pub type_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchTypesParam {
    /// Name substring to search for (case-insensitive)
    pub query: String,
    /// Maximum results to return (default: 10)
    pub limit: Option<u64>,
    /// Only return published types
    pub published_only: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GroupIdParam {
    pub group_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct CategoryIdParam {
    pub category_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct BlueprintTypeIdParam {
    pub blueprint_type_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct ProductTypeIdParam {
    pub product_type_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct SolarSystemParam {
    /// Solar system ID (provide this or name)
    pub system_id: Option<u64>,
    /// Solar system name (provide this or system_id)
    pub name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchParam {
    pub query: String,
    pub limit: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RegionParam {
    pub region_id: Option<u64>,
    pub name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ConstellationIdParam {
    pub constellation_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct StationIdParam {
    pub station_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct RouteParam {
    pub from_system_id: u64,
    pub to_system_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct MarketGroupIdParam {
    pub market_group_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct AttributeIdParam {
    pub attribute_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct EffectIdParam {
    pub effect_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct FactionIdParam {
    pub faction_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct CorporationIdParam {
    pub corporation_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct SkinIdParam {
    pub skin_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct TypeDogmaParam {
    /// EVE type ID
    pub type_id: u64,
    /// Join attributeID→name and decode skill-prerequisite attrs (182/183/184 + levels)
    /// into a `requiredSkill` object. Default false keeps the raw record.
    pub resolve_names: Option<bool>,
}

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
pub struct ModifierQueryParam {
    /// Direction-a: a skill/ship/type ID → the attributes it modifies + magnitudes
    pub type_id: Option<u64>,
    /// Direction-b: a target attribute ID (e.g. 77 miningAmount) → the modifiers that hit it
    pub attribute_id: Option<u64>,
    /// Direction-c: a dogma effect ID → the raw modifierInfo entries it defines
    pub effect_id: Option<u64>,
    /// Direction-d (with type_id): true → list EVERY attribute on the type and, per
    /// attribute, how many things modify it + the distinct modifying sources
    /// (skills first). Use this to enumerate all of a module's tunable levers before
    /// judging which matter — don't assume one "obvious" attribute is the whole story.
    pub levers: Option<bool>,
    /// Join attribute IDs to human names from dogmaAttributes (default true)
    pub resolve_names: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct TypeIdsParam {
    /// EVE type IDs to fetch in one call
    pub type_ids: Vec<u64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ResolveTypesParam {
    /// Type IDs to resolve to names
    pub type_ids: Option<Vec<u64>>,
    /// Exact type names to resolve to IDs (case-insensitive)
    pub names: Option<Vec<String>>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SkillSpParam {
    /// Skill rank directly (skillTimeConstant, attribute 275)
    pub rank: Option<u64>,
    /// Or a skill's type ID — its rank is looked up from dogma
    pub type_id: Option<u64>,
}

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

// ── Server ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct SdeMcpServer {
    pub store: Arc<SdeStore>,
    pub language: Option<String>,
}

impl SdeMcpServer {
    pub fn new(store: Arc<SdeStore>, language: Option<String>) -> Self {
        Self { store, language }
    }

    fn filter(&self, value: &mut serde_json::Value) {
        if let Some(ref lang) = self.language {
            query::apply_language_filter(value, lang);
        }
    }

    fn fetch_filtered(
        &self,
        index: &crate::store::SdeIndex,
        id: u64,
        label: &str,
    ) -> Result<String, ErrorData> {
        let mut val = query::fetch_by_id(index, id).map_err(|_| {
            ErrorData::invalid_params(format!("ID {id} not found in {label}"), None)
        })?;
        self.filter(&mut val);
        Ok(serde_json::to_string(&val).unwrap())
    }

    fn search_filtered(
        &self,
        index: &crate::store::SdeIndex,
        q: &str,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>, ErrorData> {
        let mut results = query::search_by_name(index, q, limit)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        for v in &mut results {
            self.filter(v);
        }
        Ok(results)
    }

    /// English (or configured-language) name of a type, or None if unknown.
    fn type_name(&self, id: u64) -> Option<String> {
        let v = query::fetch_by_id(&self.store.types, id).ok()?;
        pick_name(v.get("name"), self.language.as_deref())
    }

    /// Name of a dogma attribute by id, or None.
    fn attribute_name(&self, id: u64) -> Option<String> {
        let v = query::fetch_by_id(&self.store.dogma_attributes, id).ok()?;
        pick_name(v.get("name"), self.language.as_deref())
    }

    /// True if a type is a skill (category 16), via type→group→category. Used by the
    /// levers view to float skills above implants/boosters/ships when listing what
    /// modifies an attribute — the skill sources are what a training plan cares about.
    fn is_skill(&self, type_id: u64) -> bool {
        let Ok(t) = query::fetch_by_id(&self.store.types, type_id) else {
            return false;
        };
        let Some(group_id) = t.get("groupID").and_then(|x| x.as_u64()) else {
            return false;
        };
        query::fetch_by_id(&self.store.groups, group_id)
            .ok()
            .and_then(|g| g.get("categoryID").and_then(|x| x.as_u64()))
            == Some(16)
    }

    /// In-place: annotate each dogmaAttribute with `attributeName`, and decode
    /// skill-prerequisite slots (182/183/184 + 277/278/279) into a `requiredSkill` object.
    fn annotate_dogma_names(&self, val: &mut serde_json::Value) {
        let Some(attrs) = val
            .get_mut("dogmaAttributes")
            .and_then(|a| a.as_array_mut())
        else {
            return;
        };
        // First pass: collect prereq slot levels so we can pair skill id with its level.
        let mut levels: HashMap<u64, u64> = HashMap::new();
        for a in attrs.iter() {
            if let (Some(aid @ 277..=279), Some(v)) = (
                a.get("attributeID").and_then(|x| x.as_u64()),
                a.get("value").and_then(|x| x.as_f64()),
            ) {
                levels.insert(aid, v as u64);
            }
        }
        for a in attrs.iter_mut() {
            let Some(aid) = a.get("attributeID").and_then(|x| x.as_u64()) else {
                continue;
            };
            if let Some(name) = self.attribute_name(aid) {
                a["attributeName"] = serde_json::Value::String(name);
            }
            // 182→277, 183→278, 184→279 are requiredSkillN / requiredSkillNLevel pairs.
            let level_attr = match aid {
                182 => Some(277),
                183 => Some(278),
                184 => Some(279),
                _ => None,
            };
            if let (Some(level_attr), Some(skill_id)) =
                (level_attr, a.get("value").and_then(|x| x.as_f64()))
            {
                let skill_id = skill_id as u64;
                a["requiredSkill"] = serde_json::json!({
                    "skill_id": skill_id,
                    "skill_name": self.type_name(skill_id),
                    "level": levels.get(&level_attr).copied().unwrap_or(0),
                });
            }
        }
    }

    /// Direction-a: a type's outgoing modifiers — for each effect it carries, the
    /// attributes that effect modifies and the magnitude on this type.
    fn modifiers_for_type(
        &self,
        type_id: u64,
        resolve_names: bool,
    ) -> Result<serde_json::Value, ErrorData> {
        let dogma = query::fetch_by_id(&self.store.type_dogma, type_id).map_err(|_| {
            ErrorData::invalid_params(format!("ID {type_id} not found in typeDogma"), None)
        })?;
        // Magnitude lookup: this type's own attribute values, keyed by attributeID.
        let mut magnitudes: HashMap<u64, f64> = HashMap::new();
        if let Some(attrs) = dogma.get("dogmaAttributes").and_then(|a| a.as_array()) {
            for a in attrs {
                if let (Some(aid), Some(v)) = (
                    a.get("attributeID").and_then(|x| x.as_u64()),
                    a.get("value").and_then(|x| x.as_f64()),
                ) {
                    magnitudes.insert(aid, v);
                }
            }
        }
        let mut rows = Vec::new();
        if let Some(effects) = dogma.get("dogmaEffects").and_then(|e| e.as_array()) {
            for e in effects {
                let Some(eid) = e.get("effectID").and_then(|x| x.as_u64()) else {
                    tracing::warn!(
                        "type {type_id}: dogmaEffects entry has non-integer effectID; skipping"
                    );
                    continue;
                };
                let effect = match query::fetch_by_id(&self.store.dogma_effects, eid) {
                    Ok(effect) => effect,
                    // Effect not indexed at all: nothing to resolve, expected skip.
                    Err(_) if !self.store.dogma_effects.id_index.contains_key(&eid) => continue,
                    // Indexed but unreadable (IO/parse): a real failure — don't let it
                    // masquerade as "this type modifies nothing".
                    Err(err) => {
                        tracing::warn!(
                            "type {type_id}: failed to read dogma effect {eid}: {err}; \
                             modifier list may be incomplete"
                        );
                        continue;
                    }
                };
                let Some(mods) = effect.get("modifierInfo").and_then(|m| m.as_array()) else {
                    continue;
                };
                for m in mods {
                    let modified = m.get("modifiedAttributeID").and_then(|x| x.as_u64());
                    let modifying = m.get("modifyingAttributeID").and_then(|x| x.as_u64());
                    let (Some(modified), Some(modifying)) = (modified, modifying) else {
                        continue;
                    };
                    let op = m.get("operation").and_then(|x| x.as_i64());
                    let mut row = serde_json::json!({
                        "effect_id": eid,
                        "modified_attribute_id": modified,
                        "modifying_attribute_id": modifying,
                        "operation": op,
                        "operation_name": op.map(operation_label),
                        "magnitude": magnitudes.get(&modifying),
                    });
                    if resolve_names {
                        row["modified_attribute_name"] =
                            serde_json::to_value(self.attribute_name(modified)).unwrap();
                        row["modifying_attribute_name"] =
                            serde_json::to_value(self.attribute_name(modifying)).unwrap();
                    }
                    rows.push(row);
                }
            }
        }
        Ok(serde_json::json!({"type_id": type_id, "modifies": rows}))
    }

    /// Direction-c: the raw modifierInfo entries a single dogma effect defines.
    fn modifiers_for_effect(
        &self,
        effect_id: u64,
        resolve_names: bool,
    ) -> Result<serde_json::Value, ErrorData> {
        let effect = query::fetch_by_id(&self.store.dogma_effects, effect_id).map_err(|_| {
            ErrorData::invalid_params(format!("ID {effect_id} not found in dogmaEffects"), None)
        })?;
        let mut rows = Vec::new();
        if let Some(mods) = effect.get("modifierInfo").and_then(|m| m.as_array()) {
            for m in mods {
                let modified = m.get("modifiedAttributeID").and_then(|x| x.as_u64());
                let modifying = m.get("modifyingAttributeID").and_then(|x| x.as_u64());
                let op = m.get("operation").and_then(|x| x.as_i64());
                let mut row = serde_json::json!({
                    "modified_attribute_id": modified,
                    "modifying_attribute_id": modifying,
                    "operation": op,
                    "operation_name": op.map(operation_label),
                    "func": m.get("func"),
                    "domain": m.get("domain"),
                    "skill_type_id": m.get("skillTypeID").and_then(|x| x.as_u64()),
                });
                if resolve_names {
                    if let Some(a) = modified {
                        row["modified_attribute_name"] =
                            serde_json::to_value(self.attribute_name(a)).unwrap();
                    }
                    if let Some(a) = modifying {
                        row["modifying_attribute_name"] =
                            serde_json::to_value(self.attribute_name(a)).unwrap();
                    }
                }
                rows.push(row);
            }
        }
        Ok(serde_json::json!({"effect_id": effect_id, "modifiers": rows}))
    }

    /// Direction-b: which modifiers target a given attribute (reverse index lookup).
    fn modifiers_for_attribute(
        &self,
        attribute_id: u64,
        resolve_names: bool,
    ) -> Result<serde_json::Value, ErrorData> {
        // Distinguish "valid attribute nobody modifies" (empty list) from "no such
        // attribute" (error) — otherwise a typo'd id returns a confident empty answer.
        if !self
            .store
            .dogma_attributes
            .id_index
            .contains_key(&attribute_id)
        {
            return Err(ErrorData::invalid_params(
                format!("ID {attribute_id} not found in dogmaAttributes"),
                None,
            ));
        }
        // Cap owners surfaced per effect: a few generic effects are owned by hundreds
        // of types and would otherwise swamp the response. owner_count makes any
        // truncation explicit (no silent cap).
        const MAX_OWNERS: usize = 25;
        let mut rows = Vec::new();
        if let Some(mods) = self.store.attribute_modifiers.get(&attribute_id) {
            for m in mods {
                // The real bonus source is the type whose dogmaEffects own this effect
                // — NOT modifierInfo.skillTypeID (that's a required-skill filter on the
                // boosted modules). One effect can be owned by several types (e.g. 391
                // is owned by both Mining and Astrogeology), so emit one row per owner.
                let owners = self.store.effect_to_types.get(&m.effect_id);
                let owner_count = owners.map_or(0, |o| o.len());
                // Build the per-owner row. `source` is the owning type (or None for an
                // orphan effect no type references — still surfaced so it isn't dropped).
                let push_row = |source: Option<u64>| {
                    // Magnitude is the modifying attribute's value on the *owning* type.
                    let magnitude = source.and_then(|sid| {
                        query::fetch_by_id(&self.store.type_dogma, sid)
                            .ok()
                            .and_then(|d| {
                                d.get("dogmaAttributes")
                                    .and_then(|a| a.as_array())
                                    .and_then(|attrs| {
                                        attrs.iter().find_map(|a| {
                                            let aid =
                                                a.get("attributeID").and_then(|x| x.as_u64())?;
                                            (aid == m.modifying_attribute_id)
                                                .then(|| a.get("value").and_then(|x| x.as_f64()))
                                                .flatten()
                                        })
                                    })
                            })
                    });
                    let mut row = serde_json::json!({
                        "effect_id": m.effect_id,
                        "modified_attribute_id": m.modified_attribute_id,
                        "modifying_attribute_id": m.modifying_attribute_id,
                        "operation": m.operation,
                        "operation_name": operation_label(m.operation),
                        "func": m.func,
                        "source_type_id": source,
                        "required_skill_id": m.skill_type_id,
                        "magnitude": magnitude,
                    });
                    if owner_count > MAX_OWNERS {
                        row["owner_count"] = serde_json::json!(owner_count);
                    }
                    if resolve_names {
                        row["modifying_attribute_name"] =
                            serde_json::to_value(self.attribute_name(m.modifying_attribute_id))
                                .unwrap();
                        if let Some(sid) = source {
                            row["source_type_name"] =
                                serde_json::to_value(self.type_name(sid)).unwrap();
                        }
                        if let Some(sid) = m.skill_type_id {
                            row["required_skill_name"] =
                                serde_json::to_value(self.type_name(sid)).unwrap();
                        }
                    }
                    row
                };
                match owners {
                    Some(types) if !types.is_empty() => {
                        for &sid in types.iter().take(MAX_OWNERS) {
                            rows.push(push_row(Some(sid)));
                        }
                    }
                    // No owning type references this effect: still surface the modifier.
                    _ => rows.push(push_row(None)),
                }
            }
        }
        Ok(serde_json::json!({"attribute_id": attribute_id, "modified_by": rows}))
    }

    /// Direction-d: a module-centric "all tunable levers" view. For the type's every
    /// dogmaAttribute, report how many things modify it and the distinct modifying
    /// sources (skills first, so a skill lever never gets truncated behind implant /
    /// booster noise). Built to defeat the under-enumeration failure mode where an
    /// agent anchors on one obvious attribute (e.g. miningAmount) and misses the
    /// others that also feed effective output (crit chance, crit yield, duration).
    /// This is a SUMMARY: drill into any attribute with attribute_id for full rows.
    fn levers_for_type(
        &self,
        type_id: u64,
        resolve_names: bool,
    ) -> Result<serde_json::Value, ErrorData> {
        // Distinct sources listed per attribute before truncation — bounds output
        // while keeping every skill lever visible (skills are sorted to the front).
        const MAX_SOURCES: usize = 12;
        let dogma = query::fetch_by_id(&self.store.type_dogma, type_id).map_err(|_| {
            ErrorData::invalid_params(format!("ID {type_id} not found in typeDogma"), None)
        })?;
        // Required-skill applicability filter. A LocationRequiredSkill / OwnerRequiredSkill
        // modifier only hits modules that REQUIRE its skillTypeID. The reverse map is
        // global, so without this an afterburner-duration or turret-range skill would
        // show up as a "lever" on a mining laser. Keep modifiers whose skill_type_id is
        // None (unconditional / item-level) or is one of THIS type's required skills.
        let required_skills: std::collections::HashSet<u64> = dogma
            .get("dogmaAttributes")
            .and_then(|a| a.as_array())
            .map(|attrs| {
                attrs
                    .iter()
                    .filter_map(|a| {
                        let aid = a.get("attributeID").and_then(|x| x.as_u64())?;
                        // 182/183/184 carry requiredSkill1..3 as the skill's type id.
                        matches!(aid, 182..=184)
                            .then(|| a.get("value").and_then(|x| x.as_f64()))
                            .flatten()
                            .map(|v| v as u64)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let applies = |m: &crate::store::ModifierRef| {
            m.skill_type_id
                .is_none_or(|sid| required_skills.contains(&sid))
        };
        let mut attributes = Vec::new();
        if let Some(attrs) = dogma.get("dogmaAttributes").and_then(|a| a.as_array()) {
            for a in attrs {
                let Some(aid) = a.get("attributeID").and_then(|x| x.as_u64()) else {
                    continue;
                };
                let value = a.get("value").and_then(|x| x.as_f64());

                // Gather distinct owning-type sources across every modifier hitting
                // this attribute (owner = the type whose dogmaEffects own the effect),
                // keeping only modifiers that actually apply to this module.
                let mut source_ids: Vec<u64> = Vec::new();
                let mut modifier_count = 0usize;
                if let Some(mods) = self.store.attribute_modifiers.get(&aid) {
                    for m in mods.iter().filter(|m| applies(m)) {
                        let owners = self.store.effect_to_types.get(&m.effect_id);
                        match owners {
                            Some(types) if !types.is_empty() => {
                                modifier_count += types.len();
                                for &sid in types {
                                    if !source_ids.contains(&sid) {
                                        source_ids.push(sid);
                                    }
                                }
                            }
                            _ => modifier_count += 1,
                        }
                    }
                }
                // Skills first, then by id for determinism, so the training-relevant
                // levers survive the MAX_SOURCES cap even amid many item sources.
                let mut sources: Vec<(u64, bool)> = source_ids
                    .iter()
                    .map(|&id| (id, self.is_skill(id)))
                    .collect();
                sources.sort_by(|x, y| y.1.cmp(&x.1).then(x.0.cmp(&y.0)));
                let source_count = sources.len();
                let truncated = source_count > MAX_SOURCES;
                let source_rows: Vec<_> = sources
                    .iter()
                    .take(MAX_SOURCES)
                    .map(|&(id, is_skill)| {
                        let mut s = serde_json::json!({"type_id": id, "is_skill": is_skill});
                        if resolve_names {
                            s["name"] = serde_json::to_value(self.type_name(id)).unwrap();
                        }
                        s
                    })
                    .collect();

                let mut entry = serde_json::json!({
                    "attribute_id": aid,
                    "value": value,
                    "modifier_count": modifier_count,
                    "source_count": source_count,
                    "sources": source_rows,
                });
                if truncated {
                    entry["sources_truncated"] = serde_json::json!(true);
                }
                if resolve_names {
                    entry["attribute_name"] =
                        serde_json::to_value(self.attribute_name(aid)).unwrap();
                }
                attributes.push((modifier_count, entry));
            }
        }
        // Most-modified attributes first: the tunable levers float to the top, fixed
        // stats (modifier_count 0) sink — but all stay present (no silent omission).
        attributes.sort_by_key(|y| std::cmp::Reverse(y.0));
        let attributes: Vec<_> = attributes.into_iter().map(|(_, e)| e).collect();
        Ok(serde_json::json!({"type_id": type_id, "attributes": attributes}))
    }
}

// ── Tool implementations ─────────────────────────────────────────────────────

#[tool_router]
impl SdeMcpServer {
    #[tool(
        description = "Get SDE metadata: build number, release date, data directory, files scanned"
    )]
    async fn sde_status(&self) -> String {
        serde_json::to_string(&serde_json::json!({
            "build": self.store.build,
            "release_date": self.store.release_date,
            "data_dir": self.store.data_dir.display().to_string(),
            "files_scanned": self.store.files_scanned,
            "last_updated": self.store.last_updated,
        }))
        .unwrap()
    }

    #[tool(description = "Get a type (item) by its type ID")]
    async fn sde_get_type(
        &self,
        Parameters(p): Parameters<TypeIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.types, p.type_id, "types")
    }

    #[tool(description = "Search types by name substring; optionally filter to published only")]
    async fn sde_search_types(
        &self,
        Parameters(p): Parameters<SearchTypesParam>,
    ) -> Result<String, ErrorData> {
        let limit = p.limit.unwrap_or(10) as usize;
        let published_only = p.published_only.unwrap_or(false);
        let mut results = self.search_filtered(&self.store.types, &p.query, limit)?;
        if published_only {
            results.retain(|v| {
                v.get("published")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(false)
            });
        }
        Ok(serde_json::to_string(&results).unwrap())
    }

    #[tool(description = "Get a type group by its group ID")]
    async fn sde_get_group(
        &self,
        Parameters(p): Parameters<GroupIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.groups, p.group_id, "groups")
    }

    #[tool(description = "Get a type category by its category ID")]
    async fn sde_get_category(
        &self,
        Parameters(p): Parameters<CategoryIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.categories, p.category_id, "categories")
    }

    #[tool(description = "Get the materials required to reprocess a type")]
    async fn sde_get_type_materials(
        &self,
        Parameters(p): Parameters<TypeIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.type_materials, p.type_id, "typeMaterials")
    }

    #[tool(
        description = "Get the dogma attributes and effects of a type by its type ID (e.g. skill rank/skillTimeConstant attribute 275, module stats). Set resolve_names to annotate each attribute with its name and decode skill-prerequisite attributes into readable requiredSkill objects."
    )]
    async fn sde_get_type_dogma(
        &self,
        Parameters(p): Parameters<TypeDogmaParam>,
    ) -> Result<String, ErrorData> {
        let mut val = query::fetch_by_id(&self.store.type_dogma, p.type_id).map_err(|_| {
            ErrorData::invalid_params(format!("ID {} not found in typeDogma", p.type_id), None)
        })?;
        if p.resolve_names.unwrap_or(false) {
            self.annotate_dogma_names(&mut val);
        }
        self.filter(&mut val);
        Ok(serde_json::to_string(&val).unwrap())
    }

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
        description = "Plan how to manufacture / build / produce a Type (ship, module, component, …): the FIRST tool to call for 'how do I build X', 'what do I need to make X', 'bill of materials', or 'production chain'. Classifies the whole build tree and returns: whether the target is buildable (and its material-efficiency mode), the distinct decomposable origins present (manufactured vs reaction-output), per-origin buy-vs-build decision gates (each input tagged with its origin, ME mode, and required skills), the aggregate blueprint-job skills across the chain, and any out-of-scope leaves (invention or planetary-industry items you must buy). This is the classify-only router — neutral facts, no recommendations. Once the player picks what to build vs buy, call sde_get_production_chain for the resolved quantities and shopping list."
    )]
    async fn sde_build_type(
        &self,
        Parameters(p): Parameters<BuildTypeParam>,
    ) -> Result<String, ErrorData> {
        let store = Arc::clone(&self.store);
        let lang = self.language.clone();
        let target_id = p.product_type_id;
        let result = tokio::task::spawn_blocking(move || {
            manufacturing::build_type(&store, target_id, lang.as_deref())
        })
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
        let build_origins: HashSet<manufacturing::Origin> = match p.build_origins {
            Some(keys) => {
                let mut set = HashSet::new();
                for key in keys {
                    let origin = manufacturing::Origin::from_key(&key).ok_or_else(|| {
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
            None => HashSet::from([
                manufacturing::Origin::Manufactured,
                manufacturing::Origin::ReactionOutput,
            ]),
        };

        let params = manufacturing::ChainParams {
            target_id: p.product_type_id,
            runs: p.runs.unwrap_or(1),
            build_origins,
            buy_type_ids: p.buy_type_ids.unwrap_or_default().into_iter().collect(),
            me_default: p.me.unwrap_or(0),
            me_overrides: p.me_overrides.unwrap_or_default(),
        };

        let store = Arc::clone(&self.store);
        let lang = self.language.clone();
        let result = tokio::task::spawn_blocking(move || {
            manufacturing::production_chain(&store, &params, lang.as_deref())
        })
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
        .map_err(|e| ErrorData::invalid_params(e, None))?;
        Ok(serde_json::to_string(&result).unwrap())
    }

    #[tool(
        description = "Resolve dogma modifiers with no prose parsing. Provide exactly one of: type_id (the attributes this skill/ship/module modifies), attribute_id (which skills/ships modify this attribute), or effect_id (the raw modifierInfo entries a dogma effect defines). Magnitudes come from the source type's dogmaAttributes. For attribute_id, each row gives source_type_id/source_type_name = the type that OWNS the effect (the actual bonus source, e.g. Astrogeology), one row per owning type; required_skill_id/required_skill_name is a separate required-skill FILTER on the boosted modules (e.g. Mining) — do NOT treat it as the source. operation_name decodes the operation int: 'postPercent' means magnitude is +x% PER stacking source (NOT a flat add), 'modAdd' is flat additive, 'postMul'/'preMul' multiply — read it before interpreting magnitude. To assess what affects a MODULE's effective output (yield/DPS/tank/etc.), call with type_id + levers:true FIRST: it lists every attribute on the module with a modifier_count and the distinct modifying sources (skills first), filtered to modifiers that actually apply to this module (by its required skills), so you enumerate ALL tunable levers (crit chance, duration, etc.) before deciding which matter — never infer the full picture from one 'obvious' attribute. Then drill into a specific attribute_id for full per-source rows."
    )]
    async fn sde_get_modifiers(
        &self,
        Parameters(p): Parameters<ModifierQueryParam>,
    ) -> Result<String, ErrorData> {
        let resolve_names = p.resolve_names.unwrap_or(true);
        // Direction-d: levers mode is a distinct read over type_id, so branch first.
        if p.levers == Some(true) {
            return match (p.type_id, p.attribute_id, p.effect_id) {
                (Some(id), None, None) => {
                    Ok(serde_json::to_string(&self.levers_for_type(id, resolve_names)?).unwrap())
                }
                _ => Err(ErrorData::invalid_params(
                    "levers:true requires type_id (and not attribute_id/effect_id)",
                    None,
                )),
            };
        }
        match (p.type_id, p.attribute_id, p.effect_id) {
            (Some(id), None, None) => {
                Ok(serde_json::to_string(&self.modifiers_for_type(id, resolve_names)?).unwrap())
            }
            (None, Some(attr), None) => Ok(serde_json::to_string(
                &self.modifiers_for_attribute(attr, resolve_names)?,
            )
            .unwrap()),
            (None, None, Some(eid)) => {
                Ok(serde_json::to_string(&self.modifiers_for_effect(eid, resolve_names)?).unwrap())
            }
            _ => Err(ErrorData::invalid_params(
                "Provide exactly one of type_id, attribute_id, or effect_id",
                None,
            )),
        }
    }

    #[tool(
        description = "Batch-get multiple types by ID in one call. Returns one entry per input ID in order; missing IDs are reported with found:false rather than failing the call."
    )]
    async fn sde_get_types(
        &self,
        Parameters(p): Parameters<TypeIdsParam>,
    ) -> Result<String, ErrorData> {
        let out: Vec<_> = p
            .type_ids
            .iter()
            .map(|&id| match query::fetch_by_id(&self.store.types, id) {
                Ok(mut v) => {
                    self.filter(&mut v);
                    serde_json::json!({"type_id": id, "found": true, "type": v})
                }
                Err(_) => serde_json::json!({"type_id": id, "found": false}),
            })
            .collect();
        Ok(serde_json::to_string(&out).unwrap())
    }

    #[tool(
        description = "Batch-get the dogma of multiple types by ID in one call. Returns one entry per input ID in order; missing IDs are reported with found:false."
    )]
    async fn sde_get_types_dogma(
        &self,
        Parameters(p): Parameters<TypeIdsParam>,
    ) -> Result<String, ErrorData> {
        let out: Vec<_> = p
            .type_ids
            .iter()
            .map(|&id| match query::fetch_by_id(&self.store.type_dogma, id) {
                Ok(mut v) => {
                    self.filter(&mut v);
                    serde_json::json!({"type_id": id, "found": true, "dogma": v})
                }
                Err(_) => serde_json::json!({"type_id": id, "found": false}),
            })
            .collect();
        Ok(serde_json::to_string(&out).unwrap())
    }

    #[tool(
        description = "Bulk-resolve type IDs to names and/or exact (case-insensitive) names to type IDs in one call. Lightweight id↔name mapping — use sde_search_types for substring search and sde_get_types for full records."
    )]
    async fn sde_resolve_types(
        &self,
        Parameters(p): Parameters<ResolveTypesParam>,
    ) -> Result<String, ErrorData> {
        if p.type_ids.is_none() && p.names.is_none() {
            return Err(ErrorData::invalid_params(
                "Provide type_ids and/or names",
                None,
            ));
        }
        let by_id: Vec<_> = p
            .type_ids
            .unwrap_or_default()
            .iter()
            .map(|&id| match self.type_name(id) {
                Some(name) => serde_json::json!({"type_id": id, "name": name, "found": true}),
                None => serde_json::json!({"type_id": id, "found": false}),
            })
            .collect();
        let by_name: Vec<_> = p
            .names
            .unwrap_or_default()
            .iter()
            .map(
                |name| match self.store.types.name_index.get(&name.to_lowercase()) {
                    Some(&offset) => {
                        let id = query::fetch_at_offset(&self.store.types.path, offset)
                            .ok()
                            .and_then(|v| v.get("_key").and_then(|k| k.as_u64()));
                        match id {
                            Some(id) => {
                                serde_json::json!({"name": name, "type_id": id, "found": true})
                            }
                            None => serde_json::json!({"name": name, "found": false}),
                        }
                    }
                    None => serde_json::json!({"name": name, "found": false}),
                },
            )
            .collect();
        Ok(
            serde_json::to_string(&serde_json::json!({"by_id": by_id, "by_name": by_name}))
                .unwrap(),
        )
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

    #[tool(description = "Get a blueprint by its blueprint type ID")]
    async fn sde_get_blueprint(
        &self,
        Parameters(p): Parameters<BlueprintTypeIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.blueprints, p.blueprint_type_id, "blueprints")
    }

    #[tool(
        description = "Get the blueprint that produces a given product type, tagged with the activity that makes it. Returns {\"blueprint\": {...}, \"activity\": \"manufacturing\"|\"reaction\"} — the activity tells you whether the product is manufactured or comes out of a reaction (the two are distinct production paths with different rules; reactions ignore material efficiency). {\"result\": null} means the product has no blueprint at all (a raw material you must buy/mine). For a full multi-tier bill of materials, prefer sde_build_type."
    )]
    async fn sde_get_blueprint_for_product(
        &self,
        Parameters(p): Parameters<ProductTypeIdParam>,
    ) -> Result<String, ErrorData> {
        let Some(&bp_ref) = self.store.product_to_blueprint.get(&p.product_type_id) else {
            return Ok(serde_json::json!({"result": null}).to_string());
        };
        let mut blueprint = query::fetch_by_id(&self.store.blueprints, bp_ref.blueprint_id)
            .map_err(|_| {
                ErrorData::internal_error(
                    format!("blueprint {} missing from index", bp_ref.blueprint_id),
                    None,
                )
            })?;
        self.filter(&mut blueprint);
        Ok(serde_json::json!({
            "blueprint": blueprint,
            "activity": bp_ref.activity.as_str(),
        })
        .to_string())
    }

    #[tool(description = "Get a solar system by ID or name")]
    async fn sde_get_solar_system(
        &self,
        Parameters(p): Parameters<SolarSystemParam>,
    ) -> Result<String, ErrorData> {
        match (p.system_id, p.name) {
            (Some(id), _) => {
                self.fetch_filtered(&self.store.map_solar_systems, id, "mapSolarSystems")
            }
            (None, Some(name)) => {
                let results = self.search_filtered(&self.store.map_solar_systems, &name, 1)?;
                results
                    .into_iter()
                    .next()
                    .map(|v| serde_json::to_string(&v).unwrap())
                    .ok_or_else(|| {
                        ErrorData::invalid_params(format!("Solar system '{name}' not found"), None)
                    })
            }
            (None, None) => Err(ErrorData::invalid_params("Provide system_id or name", None)),
        }
    }

    #[tool(description = "Search solar systems by name substring")]
    async fn sde_search_solar_systems(
        &self,
        Parameters(p): Parameters<SearchParam>,
    ) -> Result<String, ErrorData> {
        let limit = p.limit.unwrap_or(10) as usize;
        let results = self.search_filtered(&self.store.map_solar_systems, &p.query, limit)?;
        Ok(serde_json::to_string(&results).unwrap())
    }

    #[tool(description = "Get a region by ID or name")]
    async fn sde_get_region(
        &self,
        Parameters(p): Parameters<RegionParam>,
    ) -> Result<String, ErrorData> {
        match (p.region_id, p.name) {
            (Some(id), _) => self.fetch_filtered(&self.store.map_regions, id, "mapRegions"),
            (None, Some(name)) => {
                let results = self.search_filtered(&self.store.map_regions, &name, 1)?;
                results
                    .into_iter()
                    .next()
                    .map(|v| serde_json::to_string(&v).unwrap())
                    .ok_or_else(|| {
                        ErrorData::invalid_params(format!("Region '{name}' not found"), None)
                    })
            }
            (None, None) => Err(ErrorData::invalid_params("Provide region_id or name", None)),
        }
    }

    #[tool(description = "Get a constellation by its constellation ID")]
    async fn sde_get_constellation(
        &self,
        Parameters(p): Parameters<ConstellationIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(
            &self.store.map_constellations,
            p.constellation_id,
            "mapConstellations",
        )
    }

    #[tool(description = "Get an NPC station by its station ID")]
    async fn sde_get_npc_station(
        &self,
        Parameters(p): Parameters<StationIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.npc_stations, p.station_id, "npcStations")
    }

    #[tool(
        description = "Find the shortest route between two solar systems; returns jump count and system ID path"
    )]
    async fn sde_find_route(
        &self,
        Parameters(p): Parameters<RouteParam>,
    ) -> Result<String, ErrorData> {
        let store = Arc::clone(&self.store);
        let from = p.from_system_id;
        let to = p.to_system_id;
        let path = tokio::task::spawn_blocking(move || bfs_route(&store.stargate_graph, from, to))
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        match path {
            Some(p) => Ok(serde_json::to_string(&serde_json::json!({
                "jumps": p.len().saturating_sub(1),
                "path": p,
            }))
            .unwrap()),
            None => Err(ErrorData::invalid_params("No route found", None)),
        }
    }

    #[tool(description = "Get a market group by its market group ID")]
    async fn sde_get_market_group(
        &self,
        Parameters(p): Parameters<MarketGroupIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.market_groups, p.market_group_id, "marketGroups")
    }

    #[tool(
        description = "Get the full ancestor chain for a market group, from root to the given group"
    )]
    async fn sde_get_market_group_tree(
        &self,
        Parameters(p): Parameters<MarketGroupIdParam>,
    ) -> Result<String, ErrorData> {
        const MAX_HOPS: usize = 20;
        let mut chain = Vec::new();
        let mut id = p.market_group_id;
        loop {
            if chain.len() >= MAX_HOPS {
                return Err(ErrorData::internal_error(
                    "Market group chain exceeds 20 hops",
                    None,
                ));
            }
            let mut val = query::fetch_by_id(&self.store.market_groups, id).map_err(|_| {
                ErrorData::invalid_params(format!("ID {id} not found in marketGroups"), None)
            })?;
            self.filter(&mut val);
            let parent = val.get("parentGroupID").and_then(|v| v.as_u64());
            chain.push(val);
            match parent {
                Some(pid) => id = pid,
                None => break,
            }
        }
        chain.reverse();
        Ok(serde_json::to_string(&chain).unwrap())
    }

    #[tool(description = "Get a dogma attribute by its attribute ID")]
    async fn sde_get_dogma_attribute(
        &self,
        Parameters(p): Parameters<AttributeIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(
            &self.store.dogma_attributes,
            p.attribute_id,
            "dogmaAttributes",
        )
    }

    #[tool(description = "Get a dogma effect by its effect ID")]
    async fn sde_get_dogma_effect(
        &self,
        Parameters(p): Parameters<EffectIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.dogma_effects, p.effect_id, "dogmaEffects")
    }

    #[tool(description = "Get a faction by its faction ID")]
    async fn sde_get_faction(
        &self,
        Parameters(p): Parameters<FactionIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.factions, p.faction_id, "factions")
    }

    #[tool(description = "Get an NPC corporation by its corporation ID")]
    async fn sde_get_npc_corporation(
        &self,
        Parameters(p): Parameters<CorporationIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(
            &self.store.npc_corporations,
            p.corporation_id,
            "npcCorporations",
        )
    }

    #[tool(description = "Get a SKIN (ship SKINs) by its skin ID")]
    async fn sde_get_skin(
        &self,
        Parameters(p): Parameters<SkinIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.skins, p.skin_id, "skins")
    }
}

/// Server-level usage playbook, delivered in the initialize handshake so the model
/// reads it before choosing tools. Kept short on purpose — long instructions get skimmed.
const SERVER_INSTRUCTIONS: &str = "\
EVE Online Static Data Export (SDE) query server. Data is read-only game data indexed by ID.

Pick the most direct tool — most questions are ONE call, not a fan-out:
- \"What skills / what order to fly SHIP or use MODULE\" → sde_get_skill_plan with ALL target type IDs in one call. Its output already gives the topo-sorted prerequisite order, each skill's rank, per-level SP cost (sp_by_level), running cumulative SP, and which target needs it. Do NOT call sde_get_type_dogma or sde_get_skill_sp per skill to rebuild this.
- \"Which skills/ships boost ATTRIBUTE X (e.g. mining yield, attr 77)\" → sde_get_modifiers with attribute_id — one call returns every modifier, one row per owning type. Read source_type_id/source_type_name as the bonus SOURCE (e.g. Astrogeology); required_skill_id/required_skill_name is only a target-module filter, NOT the source. Use type_id for the inverse, effect_id for a single effect's modifierInfo.
- Known exact names → IDs → sde_resolve_types (one bulk call). Use sde_search_types only for fuzzy/unknown-name discovery.
- Several types or dogma records at once → sde_get_types / sde_get_types_dogma (batched), not many single calls.
- Decoding skill prereqs from raw dogma → pass resolve_names:true to sde_get_type_dogma instead of memorizing attribute IDs 182/277 etc.
- \"How do I build / manufacture / produce X\" or \"bill of materials / production chain\" → sde_build_type FIRST (classifies the whole build tree + buy-vs-build gates), then sde_get_production_chain for quantities. Do NOT give fitting advice (modules/tank/DPS) for a build request unless the user explicitly asks about fitting.

Not in the SDE: market prices and fitted-yield simulation (stacking penalties). Compute yield/ISK math yourself from the dogma attributes the tools return.";

#[tool_handler(name = "eve-sde-mcp", version = "0.1.0")]
impl ServerHandler for SdeMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "eve-sde-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(SERVER_INSTRUCTIONS)
    }
}

// ── BFS route ────────────────────────────────────────────────────────────────

fn bfs_route(graph: &HashMap<u64, Vec<u64>>, from: u64, to: u64) -> Option<Vec<u64>> {
    if from == to {
        return Some(vec![from]);
    }
    let mut queue = std::collections::VecDeque::new();
    let mut prev: HashMap<u64, u64> = HashMap::new();
    queue.push_back(from);
    prev.insert(from, from);
    while let Some(curr) = queue.pop_front() {
        if let Some(neighbors) = graph.get(&curr) {
            for &next in neighbors {
                if let std::collections::hash_map::Entry::Vacant(e) = prev.entry(next) {
                    e.insert(curr);
                    if next == to {
                        let mut path = vec![to];
                        let mut node = to;
                        while node != from {
                            node = prev[&node];
                            path.push(node);
                        }
                        path.reverse();
                        return Some(path);
                    }
                    queue.push_back(next);
                }
            }
        }
    }
    None
}

// ── Skill plan ───────────────────────────────────────────────────────────────

use serde_json::Value;

const ATTR_RANK: u64 = 275; // skillTimeConstant
const PREREQ_SLOTS: [(u64, u64); 3] = [(182, 277), (183, 278), (184, 279)]; // (skillID, levelID)
const MAX_SKILL_DEPTH: usize = 12;

/// Pick the English (or requested-language) string from a localized name field,
/// tolerating both `{"en": "X"}` objects and already-filtered plain strings.
fn pick_name(name: Option<&Value>, lang: Option<&str>) -> Option<String> {
    match name {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Object(m)) => lang
            .and_then(|l| m.get(l))
            .or_else(|| m.get("en"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
        _ => None,
    }
}

/// Human-readable name for a dogma modifier `operation` code (EVE's canonical
/// dogma Operator enum). The magnitude alone is ambiguous — e.g. op 6 with
/// magnitude 5.0 is "+5% per stacking source", NOT "+5 flat". Surfacing this
/// stops callers misreading a percent bonus as additive (the exact slip that made
/// a benchmark agent treat Mining/Astrogeology's +5%/level as +5 m³/level).
fn operation_label(op: i64) -> &'static str {
    match op {
        -1 => "preAssignment (set, applied first)",
        0 => "preMul (multiply)",
        1 => "preDiv (divide)",
        2 => "modAdd (additive, flat)",
        3 => "modSub (subtractive, flat)",
        4 => "postMul (multiply)",
        5 => "postDiv (divide)",
        6 => "postPercent (+magnitude% per stacking source)",
        7 => "postAssignment (set, applied last)",
        _ => "unknown",
    }
}

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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
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

    fn make_index(content: &str) -> (tempfile::NamedTempFile, crate::store::SdeIndex) {
        let (_f, path) = write_fixture(content);
        let pb = indicatif::ProgressBar::hidden();
        let idx = crate::scan::scan_index_pub(&path, &pb).unwrap();
        (_f, idx)
    }

    fn make_server() -> SdeMcpServer {
        SdeMcpServer::new(Arc::new(default_store()), None)
    }

    fn empty_index() -> crate::store::SdeIndex {
        crate::store::SdeIndex {
            path: std::path::PathBuf::from("/dev/null"),
            id_index: HashMap::new(),
            name_index: HashMap::new(),
        }
    }

    fn default_store() -> SdeStore {
        SdeStore {
            data_dir: std::path::PathBuf::from("/tmp"),
            build: 42,
            release_date: "2024-01-01".to_string(),
            files_scanned: 17,
            last_updated: "2024-01-01".to_string(),
            types: empty_index(),
            groups: empty_index(),
            categories: empty_index(),
            blueprints: empty_index(),
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
            product_to_blueprint: HashMap::new(),
            stargate_graph: HashMap::new(),
            attribute_modifiers: HashMap::new(),
            effect_to_types: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn sde_status_returns_build_metadata() {
        let server = make_server();
        let result = server.sde_status().await;
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["build"], 42);
        assert_eq!(v["release_date"], "2024-01-01");
        assert_eq!(v["files_scanned"], 17);
    }

    #[tokio::test]
    async fn sde_get_type_returns_error_for_missing_id() {
        let server = make_server();
        let result = server
            .sde_get_type(Parameters(TypeIdParam { type_id: 99 }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("99"));
    }

    #[tokio::test]
    async fn bfs_route_finds_direct_connection() {
        let mut graph = HashMap::new();
        graph.insert(1, vec![2]);
        graph.insert(2, vec![1]);
        let path = bfs_route(&graph, 1, 2).unwrap();
        assert_eq!(path, vec![1, 2]);
    }

    #[tokio::test]
    async fn bfs_route_returns_none_for_unreachable() {
        let graph = HashMap::new();
        assert!(bfs_route(&graph, 1, 2).is_none());
    }

    #[tokio::test]
    async fn bfs_route_same_system() {
        let graph = HashMap::new();
        let path = bfs_route(&graph, 42, 42).unwrap();
        assert_eq!(path, vec![42]);
    }

    #[tokio::test]
    async fn sde_get_type_returns_record_for_known_id() {
        let (_f, types) =
            make_index("{\"_key\":34,\"name\":{\"en\":\"Tritanium\"},\"published\":true}\n");
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                types,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_type(Parameters(TypeIdParam { type_id: 34 }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 34);
    }

    #[tokio::test]
    async fn sde_search_types_returns_matches() {
        let (_f, types) = make_index(
            "{\"_key\":34,\"name\":{\"en\":\"Tritanium\"},\"published\":true}\n\
             {\"_key\":35,\"name\":{\"en\":\"Pyerite\"},\"published\":false}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                types,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_search_types(Parameters(SearchTypesParam {
                query: "trit".to_string(),
                limit: None,
                published_only: None,
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0]["_key"], 34);
    }

    #[tokio::test]
    async fn sde_search_types_published_only_filters_unpublished() {
        let (_f, types) = make_index(
            "{\"_key\":34,\"name\":{\"en\":\"Tritanium\"},\"published\":true}\n\
             {\"_key\":35,\"name\":{\"en\":\"Tritan Scrap\"},\"published\":false}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                types,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_search_types(Parameters(SearchTypesParam {
                query: "tritan".to_string(),
                limit: None,
                published_only: Some(true),
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0]["_key"], 34);
    }

    #[tokio::test]
    async fn sde_get_group_returns_record_for_known_id() {
        let (_f, groups) =
            make_index("{\"_key\":18,\"name\":{\"en\":\"Mineral\"},\"categoryID\":4}\n");
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                groups,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_group(Parameters(GroupIdParam { group_id: 18 }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 18);
        assert_eq!(v["categoryID"], 4);
    }

    #[tokio::test]
    async fn sde_get_group_returns_error_for_missing_id() {
        let server = make_server();
        let result = server
            .sde_get_group(Parameters(GroupIdParam { group_id: 99 }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("99"));
    }

    #[tokio::test]
    async fn sde_get_category_returns_record_for_known_id() {
        let (_f, categories) =
            make_index("{\"_key\":4,\"name\":{\"en\":\"Material\"},\"published\":true}\n");
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                categories,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_category(Parameters(CategoryIdParam { category_id: 4 }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 4);
    }

    #[tokio::test]
    async fn sde_get_category_returns_error_for_missing_id() {
        let server = make_server();
        let result = server
            .sde_get_category(Parameters(CategoryIdParam { category_id: 99 }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("99"));
    }

    #[tokio::test]
    async fn sde_get_type_materials_returns_record_for_known_id() {
        let (_f, type_materials) =
            make_index("{\"_key\":34,\"materials\":[{\"typeID\":35,\"quantity\":10}]}\n");
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                type_materials,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_type_materials(Parameters(TypeIdParam { type_id: 34 }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 34);
        assert_eq!(v["materials"][0]["typeID"], 35);
    }

    #[tokio::test]
    async fn sde_get_type_materials_returns_error_for_missing_id() {
        let server = make_server();
        let result = server
            .sde_get_type_materials(Parameters(TypeIdParam { type_id: 99 }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("99"));
    }

    #[tokio::test]
    async fn sde_get_type_dogma_returns_record_for_known_id() {
        let (_f, type_dogma) = make_index(
            "{\"_key\":3386,\"dogmaAttributes\":[{\"attributeID\":275,\"value\":1.0}]}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                type_dogma,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_type_dogma(Parameters(TypeDogmaParam {
                type_id: 3386,
                resolve_names: None,
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 3386);
        assert_eq!(v["dogmaAttributes"][0]["attributeID"], 275);
        assert_eq!(v["dogmaAttributes"][0]["value"], 1.0);
    }

    #[tokio::test]
    async fn sde_get_type_dogma_returns_error_for_missing_id() {
        let server = make_server();
        let result = server
            .sde_get_type_dogma(Parameters(TypeDogmaParam {
                type_id: 99,
                resolve_names: None,
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("99"));
    }

    #[tokio::test]
    async fn mcp_handshake_initialize_and_list_tools() -> anyhow::Result<()> {
        use rmcp::{ClientHandler, ServiceExt as _, model::ClientInfo};

        #[derive(Clone, Default)]
        struct DummyClient;
        impl ClientHandler for DummyClient {
            fn get_info(&self) -> ClientInfo {
                ClientInfo::default()
            }
        }

        let (server_transport, client_transport) = tokio::io::duplex(65536);
        let store = make_server().store;
        let server_handle = tokio::spawn(async move {
            SdeMcpServer::new(store, None)
                .serve(server_transport)
                .await?
                .waiting()
                .await?;
            anyhow::Ok(())
        });
        let client = DummyClient.serve(client_transport).await?;
        let tools = client.list_all_tools().await?;
        assert!(tools.len() >= 28, "expected ≥28 tools, got {}", tools.len());
        let names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert!(names.contains(&"sde_status"));
        assert!(names.contains(&"sde_find_route"));
        assert!(names.contains(&"sde_get_market_group_tree"));
        assert!(names.contains(&"sde_get_type_dogma"));
        assert!(names.contains(&"sde_get_skill_plan"));
        assert!(names.contains(&"sde_get_modifiers"));
        assert!(names.contains(&"sde_get_types"));
        assert!(names.contains(&"sde_get_types_dogma"));
        assert!(names.contains(&"sde_resolve_types"));
        assert!(names.contains(&"sde_get_skill_sp"));
        client.cancel().await?;
        let _ = server_handle.await;
        Ok(())
    }

    #[tokio::test]
    async fn sde_get_solar_system_by_id_returns_record() {
        let (_f, map_solar_systems) = make_index(
            "{\"_key\":30000142,\"name\":{\"en\":\"Jita\"},\"securityStatus\":0.9459}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                map_solar_systems,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_solar_system(Parameters(SolarSystemParam {
                system_id: Some(30000142),
                name: None,
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 30000142);
    }

    #[tokio::test]
    async fn sde_get_solar_system_by_name_returns_record() {
        let (_f, map_solar_systems) = make_index(
            "{\"_key\":30000142,\"name\":{\"en\":\"Jita\"},\"securityStatus\":0.9459}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                map_solar_systems,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_solar_system(Parameters(SolarSystemParam {
                system_id: None,
                name: Some("Jita".to_string()),
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 30000142);
    }

    #[tokio::test]
    async fn sde_find_route_returns_path_with_correct_jump_count() {
        // A → B → C → D: 3 jumps, 4 systems
        let mut graph = HashMap::new();
        graph.insert(1u64, vec![2u64]);
        graph.insert(2u64, vec![1u64, 3u64]);
        graph.insert(3u64, vec![2u64, 4u64]);
        graph.insert(4u64, vec![3u64]);
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                stargate_graph: graph,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_find_route(Parameters(RouteParam {
                from_system_id: 1,
                to_system_id: 4,
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["jumps"], 3);
        assert_eq!(v["path"].as_array().unwrap().len(), 4);
        assert_eq!(v["path"][0], 1);
        assert_eq!(v["path"][3], 4);
    }

    #[tokio::test]
    async fn sde_find_route_returns_error_for_unreachable_system() {
        let mut graph = HashMap::new();
        graph.insert(1u64, vec![2u64]);
        graph.insert(2u64, vec![1u64]);
        // system 99 is isolated
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                stargate_graph: graph,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_find_route(Parameters(RouteParam {
                from_system_id: 1,
                to_system_id: 99,
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("No route found"));
    }

    fn make_blueprint_index(
        content: &str,
    ) -> (
        tempfile::NamedTempFile,
        crate::store::SdeIndex,
        HashMap<u64, crate::store::BlueprintRef>,
    ) {
        let mut f = tempfile::Builder::new()
            .suffix(".jsonl")
            .tempfile()
            .unwrap();
        use std::io::Write as _;
        f.write_all(content.as_bytes()).unwrap();
        let path = f.path().to_path_buf();
        let pb = indicatif::ProgressBar::hidden();
        let (idx, p2b) = crate::scan::scan_blueprints_pub(&path, &pb).unwrap();
        (f, idx, p2b)
    }

    #[tokio::test]
    async fn sde_get_blueprint_returns_record_for_known_id() {
        let fixture = r#"{"_key":683,"activities":{"manufacturing":{"products":[{"typeID":582,"quantity":1}],"time":6000}}}
"#;
        let (_f, blueprints, product_to_blueprint) = make_blueprint_index(fixture);
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                blueprints,
                product_to_blueprint,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_blueprint(Parameters(BlueprintTypeIdParam {
                blueprint_type_id: 683,
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 683);
    }

    #[tokio::test]
    async fn sde_get_blueprint_for_product_returns_blueprint_for_known_product() {
        let fixture = r#"{"_key":683,"activities":{"manufacturing":{"products":[{"typeID":582,"quantity":1}],"time":6000}}}
"#;
        let (_f, blueprints, product_to_blueprint) = make_blueprint_index(fixture);
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                blueprints,
                product_to_blueprint,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_blueprint_for_product(Parameters(ProductTypeIdParam {
                product_type_id: 582,
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["blueprint"]["_key"], 683);
        assert_eq!(v["activity"], "manufacturing");
    }

    #[tokio::test]
    async fn sde_get_blueprint_for_product_returns_null_for_unknown_product() {
        let server = make_server();
        let result = server
            .sde_get_blueprint_for_product(Parameters(ProductTypeIdParam {
                product_type_id: 99999,
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["result"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn sde_get_market_group_returns_record_for_known_id() {
        let (_f, market_groups) =
            make_index("{\"_key\":4,\"name\":{\"en\":\"Ships\"},\"parentGroupID\":null}\n");
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                market_groups,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_market_group(Parameters(MarketGroupIdParam { market_group_id: 4 }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 4);
    }

    #[tokio::test]
    async fn sde_get_market_group_returns_error_for_missing_id() {
        let server = make_server();
        let result = server
            .sde_get_market_group(Parameters(MarketGroupIdParam {
                market_group_id: 99,
            }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("99"));
    }

    #[tokio::test]
    async fn sde_get_market_group_tree_walks_multi_level_chain() {
        // root (id=1) → child (id=2) → grandchild (id=3)
        let fixture = concat!(
            "{\"_key\":1,\"name\":{\"en\":\"Root\"}}\n",
            "{\"_key\":2,\"name\":{\"en\":\"Child\"},\"parentGroupID\":1}\n",
            "{\"_key\":3,\"name\":{\"en\":\"Grandchild\"},\"parentGroupID\":2}\n",
        );
        let (_f, market_groups) = make_index(fixture);
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                market_groups,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_market_group_tree(Parameters(MarketGroupIdParam { market_group_id: 3 }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["_key"], 1); // root first
        assert_eq!(arr[1]["_key"], 2);
        assert_eq!(arr[2]["_key"], 3); // requested group last
    }

    #[tokio::test]
    async fn sde_get_market_group_tree_single_node_has_no_parent() {
        let (_f, market_groups) = make_index("{\"_key\":1,\"name\":{\"en\":\"Root\"}}\n");
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                market_groups,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_market_group_tree(Parameters(MarketGroupIdParam { market_group_id: 1 }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0]["_key"], 1);
    }

    #[tokio::test]
    async fn sde_get_dogma_attribute_returns_record_for_known_id() {
        let (_f, dogma_attributes) =
            make_index("{\"_key\":37,\"name\":{\"en\":\"CPU\"},\"unitID\":5}\n");
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                dogma_attributes,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_dogma_attribute(Parameters(AttributeIdParam { attribute_id: 37 }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 37);
        assert_eq!(v["unitID"], 5);
    }

    #[tokio::test]
    async fn sde_get_dogma_attribute_returns_error_for_missing_id() {
        let server = make_server();
        let result = server
            .sde_get_dogma_attribute(Parameters(AttributeIdParam { attribute_id: 99 }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("99"));
    }

    #[tokio::test]
    async fn sde_get_dogma_effect_returns_record_for_known_id() {
        let (_f, dogma_effects) =
            make_index("{\"_key\":11,\"name\":{\"en\":\"loPower\"},\"effectCategory\":0}\n");
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                dogma_effects,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_dogma_effect(Parameters(EffectIdParam { effect_id: 11 }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 11);
    }

    #[tokio::test]
    async fn sde_get_faction_returns_record_for_known_id() {
        let (_f, factions) = make_index(
            "{\"_key\":500001,\"name\":{\"en\":\"Caldari State\"},\"corporationID\":1000035}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                factions,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_faction(Parameters(FactionIdParam { faction_id: 500001 }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 500001);
    }

    #[tokio::test]
    async fn sde_get_npc_corporation_returns_record_for_known_id() {
        let (_f, npc_corporations) = make_index(
            "{\"_key\":1000035,\"name\":{\"en\":\"Caldari Navy\"},\"factionID\":500001}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                npc_corporations,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_npc_corporation(Parameters(CorporationIdParam {
                corporation_id: 1000035,
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 1000035);
        assert_eq!(v["factionID"], 500001);
    }

    #[tokio::test]
    async fn sde_get_skin_returns_record_for_known_id() {
        let (_f, skins) =
            make_index("{\"_key\":1001,\"name\":{\"en\":\"Caldari Navy SKIN\"},\"typeID\":638}\n");
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                skins,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_skin(Parameters(SkinIdParam { skin_id: 1001 }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 1001);
        assert_eq!(v["typeID"], 638);
    }

    #[tokio::test]
    async fn sde_get_skin_returns_error_for_missing_id() {
        let server = make_server();
        let result = server
            .sde_get_skin(Parameters(SkinIdParam { skin_id: 99 }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("99"));
    }

    #[tokio::test]
    async fn sde_search_solar_systems_returns_matches() {
        let (_f, map_solar_systems) = make_index(
            "{\"_key\":30000142,\"name\":{\"en\":\"Jita\"},\"securityStatus\":0.9459}\n\
             {\"_key\":30000144,\"name\":{\"en\":\"Perimeter\"},\"securityStatus\":0.9531}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                map_solar_systems,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_search_solar_systems(Parameters(SearchParam {
                query: "jit".to_string(),
                limit: None,
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0]["_key"], 30000142);
    }

    #[tokio::test]
    async fn sde_get_region_returns_record_by_id() {
        let (_f, map_regions) = make_index("{\"_key\":10000002,\"name\":{\"en\":\"The Forge\"}}\n");
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                map_regions,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_region(Parameters(RegionParam {
                region_id: Some(10000002),
                name: None,
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 10000002);
    }

    #[tokio::test]
    async fn sde_get_constellation_returns_record_for_known_id() {
        let (_f, map_constellations) = make_index(
            "{\"_key\":20000020,\"name\":{\"en\":\"Kimotoro\"},\"regionID\":10000002}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                map_constellations,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_constellation(Parameters(ConstellationIdParam {
                constellation_id: 20000020,
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 20000020);
    }

    #[tokio::test]
    async fn sde_get_npc_station_returns_record_for_known_id() {
        let (_f, npc_stations) =
            make_index("{\"_key\":60003760,\"solarSystemID\":30000142,\"ownerID\":1000035}\n");
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                npc_stations,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_npc_station(Parameters(StationIdParam {
                station_id: 60003760,
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 60003760);
    }

    #[tokio::test]
    async fn mcp_all_21_tools_via_fixture_data() -> anyhow::Result<()> {
        use rmcp::{
            ClientHandler, ServiceExt as _,
            model::{CallToolRequestParams, ClientInfo},
        };

        #[derive(Clone, Default)]
        struct DummyClient;
        impl ClientHandler for DummyClient {
            fn get_info(&self) -> ClientInfo {
                ClientInfo::default()
            }
        }

        fn text_json(result: &rmcp::model::CallToolResult) -> serde_json::Value {
            let text = result
                .content
                .first()
                .and_then(|c| c.raw.as_text())
                .map(|t| t.text.as_str())
                .expect("expected text content");
            serde_json::from_str(text).expect("invalid JSON in tool response")
        }

        fn obj(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
            v.as_object().unwrap().clone()
        }

        let fixture_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sde");
        let store = crate::scan::scan_sde(&fixture_dir, 3333874, "2024-01-15")?;

        let (server_transport, client_transport) = tokio::io::duplex(65536);
        let server_handle = tokio::spawn(async move {
            SdeMcpServer::new(store, Some("en".to_string()))
                .serve(server_transport)
                .await?
                .waiting()
                .await?;
            anyhow::Ok(())
        });
        let client = DummyClient.serve(client_transport).await?;

        // sde_status
        let r = text_json(
            &client
                .call_tool(CallToolRequestParams::new("sde_status"))
                .await?,
        );
        assert_eq!(r["build"], 3333874);
        assert_eq!(r["release_date"], "2024-01-15");
        assert!(r["files_scanned"].as_u64().unwrap() > 0);

        // sde_get_type
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_type")
                        .with_arguments(obj(serde_json::json!({"type_id": 34}))),
                )
                .await?,
        );
        assert_eq!(r["_key"], 34);
        assert_eq!(r["name"], "Tritanium");

        // sde_search_types
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_search_types")
                        .with_arguments(obj(serde_json::json!({"query": "trit"}))),
                )
                .await?,
        );
        assert!(r.as_array().unwrap().iter().any(|v| v["_key"] == 34));

        // sde_get_group
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_group")
                        .with_arguments(obj(serde_json::json!({"group_id": 18}))),
                )
                .await?,
        );
        assert_eq!(r["_key"], 18);
        assert_eq!(r["name"], "Mineral");

        // sde_get_category
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_category")
                        .with_arguments(obj(serde_json::json!({"category_id": 4}))),
                )
                .await?,
        );
        assert_eq!(r["_key"], 4);
        assert_eq!(r["name"], "Material");

        // sde_get_type_materials
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_type_materials")
                        .with_arguments(obj(serde_json::json!({"type_id": 1230}))),
                )
                .await?,
        );
        assert_eq!(r["_key"], 1230);
        assert!(r["materials"].as_array().is_some());

        // sde_get_type_dogma (Ferox has dogma attributes in fixture)
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_type_dogma")
                        .with_arguments(obj(serde_json::json!({"type_id": 16227}))),
                )
                .await?,
        );
        assert_eq!(r["_key"], 16227);
        assert!(r["dogmaAttributes"].as_array().is_some());

        // sde_get_blueprint
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_blueprint")
                        .with_arguments(obj(serde_json::json!({"blueprint_type_id": 16228}))),
                )
                .await?,
        );
        assert_eq!(r["_key"], 16228);
        assert!(r["activities"]["manufacturing"].is_object());

        // sde_get_blueprint_for_product (Ferox blueprint makes Ferox)
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_blueprint_for_product")
                        .with_arguments(obj(serde_json::json!({"product_type_id": 16227}))),
                )
                .await?,
        );
        assert_eq!(r["blueprint"]["_key"], 16228);
        assert_eq!(r["activity"], "manufacturing");

        // sde_get_solar_system (by ID)
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_solar_system")
                        .with_arguments(obj(serde_json::json!({"system_id": 30000142}))),
                )
                .await?,
        );
        assert_eq!(r["_key"], 30000142);
        assert_eq!(r["name"], "Jita");

        // sde_search_solar_systems
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_search_solar_systems")
                        .with_arguments(obj(serde_json::json!({"query": "jita"}))),
                )
                .await?,
        );
        assert!(r.as_array().unwrap().iter().any(|v| v["_key"] == 30000142));

        // sde_get_region (by ID)
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_region")
                        .with_arguments(obj(serde_json::json!({"region_id": 10000002}))),
                )
                .await?,
        );
        assert_eq!(r["_key"], 10000002);
        assert_eq!(r["name"], "The Forge");

        // sde_get_constellation
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_constellation")
                        .with_arguments(obj(serde_json::json!({"constellation_id": 20000020}))),
                )
                .await?,
        );
        assert_eq!(r["_key"], 20000020);
        assert_eq!(r["name"], "Kimotoro");

        // sde_get_npc_station
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_npc_station")
                        .with_arguments(obj(serde_json::json!({"station_id": 60003760}))),
                )
                .await?,
        );
        assert_eq!(r["_key"], 60003760);
        assert_eq!(r["solarSystemID"], 30000142);

        // sde_find_route: Jita → Perimeter (1 jump)
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_find_route").with_arguments(obj(
                        serde_json::json!({
                            "from_system_id": 30000142,
                            "to_system_id": 30000144,
                        }),
                    )),
                )
                .await?,
        );
        assert_eq!(r["jumps"], 1);
        assert_eq!(r["path"].as_array().unwrap().len(), 2);
        assert_eq!(r["path"][0], 30000142);
        assert_eq!(r["path"][1], 30000144);

        // sde_find_route: unreachable system → error response (Ikuchi has no stargates)
        let err = client
            .call_tool(
                CallToolRequestParams::new("sde_find_route").with_arguments(obj(
                    serde_json::json!({
                        "from_system_id": 30000142,
                        "to_system_id": 30000138,
                    }),
                )),
            )
            .await;
        assert!(err.is_err(), "expected error for unreachable system");

        // sde_get_market_group
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_market_group")
                        .with_arguments(obj(serde_json::json!({"market_group_id": 1857}))),
                )
                .await?,
        );
        assert_eq!(r["_key"], 1857);
        assert_eq!(r["name"], "Minerals");

        // sde_get_market_group_tree (Minerals → Materials → Manufacture & Research)
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_market_group_tree")
                        .with_arguments(obj(serde_json::json!({"market_group_id": 1857}))),
                )
                .await?,
        );
        let arr = r.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["_key"], 475); // root: Manufacture & Research
        assert_eq!(arr[2]["_key"], 1857); // leaf: Minerals

        // sde_get_dogma_attribute
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_dogma_attribute")
                        .with_arguments(obj(serde_json::json!({"attribute_id": 30}))),
                )
                .await?,
        );
        assert_eq!(r["_key"], 30);
        assert_eq!(r["name"], "power");

        // sde_get_dogma_effect
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_dogma_effect")
                        .with_arguments(obj(serde_json::json!({"effect_id": 11}))),
                )
                .await?,
        );
        assert_eq!(r["_key"], 11);
        assert_eq!(r["name"], "loPower");

        // sde_get_faction
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_faction")
                        .with_arguments(obj(serde_json::json!({"faction_id": 500001}))),
                )
                .await?,
        );
        assert_eq!(r["_key"], 500001);
        assert_eq!(r["name"], "Caldari State");

        // sde_get_npc_corporation
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_npc_corporation")
                        .with_arguments(obj(serde_json::json!({"corporation_id": 1000035}))),
                )
                .await?,
        );
        assert_eq!(r["_key"], 1000035);
        assert_eq!(r["name"], "Caldari Navy");

        // sde_get_skin
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_skin")
                        .with_arguments(obj(serde_json::json!({"skin_id": 50}))),
                )
                .await?,
        );
        assert_eq!(r["_key"], 50);
        assert_eq!(r["internalName"], "Ferox Caldari Union Day YC124");

        // sde_get_skill_plan: Covetor + ORE Deep Core Strip Miner → one merged plan
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_skill_plan").with_arguments(obj(
                        serde_json::json!({
                            "targets": [{"type_id": 17476}, {"type_id": 87562}]
                        }),
                    )),
                )
                .await?,
        );
        let plan = r["plan"].as_array().unwrap();
        // Mining deduped to level 5 (module demands 5), prereqs before dependents.
        assert_eq!(plan[0]["skill_id"], 3386);
        assert_eq!(plan[0]["required_level"], 5);
        assert_eq!(plan[0]["sp_for_level"], 256000);
        assert_eq!(plan[0]["required_by"].as_array().unwrap().len(), 2);
        assert_eq!(plan.last().unwrap()["skill_id"], 17940); // Mining Barge last
        assert_eq!(r["total_sp"], 312000);

        // sde_get_modifiers direction-b: what modifies miningAmount (77)
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_modifiers")
                        .with_arguments(obj(serde_json::json!({"attribute_id": 77}))),
                )
                .await?,
        );
        let mods = r["modified_by"].as_array().unwrap();
        // One row per owning type: effect 391 is owned by BOTH Mining (3386) and
        // Astrogeology (3410), each granting +5% via its own attr 434. The old code
        // collapsed this to a single "Mining" row and hid Astrogeology entirely.
        assert!(mods.iter().any(|m| m["source_type_id"] == 3386
            && m["source_type_name"] == "Mining"
            && m["magnitude"] == 5.0));
        assert!(
            mods.iter().any(|m| m["source_type_id"] == 3410
                && m["source_type_name"] == "Astrogeology"
                && m["magnitude"] == 5.0),
            "Astrogeology must surface as a yield source"
        );
        // operation_name decodes op 6 as percent so the +5 isn't read as flat m³.
        assert!(mods.iter().any(|m| {
            m["source_type_id"] == 3410
                && m["operation"] == 6
                && m["operation_name"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("postPercent"))
        }));
        // The skillTypeID filter is now a distinct field, not mislabeled as the source.
        assert!(
            mods.iter()
                .all(|m| m["skill_type_id"].is_null() && m["skill_name"].is_null()),
            "old skill_type_id/skill_name keys removed (renamed to required_skill_*)"
        );
        assert!(
            mods.iter()
                .any(|m| m["required_skill_id"] == 3386 && m["required_skill_name"] == "Mining")
        );

        // sde_get_modifiers direction-a: Mining skill's outgoing modifiers
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_modifiers")
                        .with_arguments(obj(serde_json::json!({"type_id": 3386}))),
                )
                .await?,
        );
        assert!(
            r["modifies"]
                .as_array()
                .unwrap()
                .iter()
                .any(|m| m["modified_attribute_id"] == 77)
        );

        // sde_get_modifiers direction-c: a dogma effect's raw modifierInfo
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_modifiers")
                        .with_arguments(obj(serde_json::json!({"effect_id": 391}))),
                )
                .await?,
        );
        let m = &r["modifiers"][0];
        assert_eq!(m["modified_attribute_id"], 77);
        assert_eq!(m["modifying_attribute_id"], 434);
        assert_eq!(m["skill_type_id"], 3386);

        // sde_get_type_dogma resolve_names: decode Mining Barge prereqs
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_type_dogma").with_arguments(obj(
                        serde_json::json!({"type_id": 17940, "resolve_names": true}),
                    )),
                )
                .await?,
        );
        let attrs = r["dogmaAttributes"].as_array().unwrap();
        assert!(
            attrs
                .iter()
                .any(|a| a["requiredSkill"]["skill_name"] == "Astrogeology"
                    && a["requiredSkill"]["level"] == 3)
        );

        // sde_get_types batch with a missing id
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_types")
                        .with_arguments(obj(serde_json::json!({"type_ids": [34, 999999]}))),
                )
                .await?,
        );
        let arr = r.as_array().unwrap();
        assert_eq!(arr[0]["found"], true);
        assert_eq!(arr[0]["type"]["name"], "Tritanium");
        assert_eq!(arr[1]["found"], false);

        // sde_resolve_types both directions
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_resolve_types").with_arguments(obj(
                        serde_json::json!({"type_ids": [87562], "names": ["Covetor", "Mining"]}),
                    )),
                )
                .await?,
        );
        assert_eq!(r["by_id"][0]["name"], "ORE Deep Core Strip Miner");
        assert_eq!(r["by_name"][0]["type_id"], 17476);
        assert_eq!(r["by_name"][1]["type_id"], 3386);

        // sde_get_skill_sp by type_id (Astrogeology = rank 3)
        let r = text_json(
            &client
                .call_tool(
                    CallToolRequestParams::new("sde_get_skill_sp")
                        .with_arguments(obj(serde_json::json!({"type_id": 3410}))),
                )
                .await?,
        );
        assert_eq!(r["rank"], 3);
        let lvls = r["levels"].as_array().unwrap();
        assert_eq!(lvls[0]["sp_to_reach"], 750); // rank3 L1 = 3 × 250
        assert_eq!(lvls[4]["sp_to_reach"], 768000); // rank3 L5 = 3 × 256000
        assert_eq!(lvls[4]["increment"], 768000 - 3 * 45255);

        client.cancel().await?;
        let _ = server_handle.await;
        Ok(())
    }

    #[test]
    fn skill_sp_matches_canonical_rank1_points() {
        assert_eq!(skill_sp(1, 1), 250);
        assert_eq!(skill_sp(1, 2), 1414);
        assert_eq!(skill_sp(1, 3), 8000);
        assert_eq!(skill_sp(1, 4), 45255);
        assert_eq!(skill_sp(1, 5), 256000);
        assert_eq!(skill_sp(3, 3), 24000); // rank scales linearly
        assert_eq!(skill_sp(1, 0), 0);
    }

    fn fixture_store() -> Arc<SdeStore> {
        let fixture_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sde");
        crate::scan::scan_sde(&fixture_dir, 3333874, "2024-01-15").unwrap()
    }

    #[test]
    fn build_skill_plan_dedupes_to_highest_level_and_topo_sorts() {
        let store = fixture_store();
        let targets = vec![
            SkillPlanTarget {
                type_id: 17476,
                level_override: None,
            }, // Covetor (ship)
            SkillPlanTarget {
                type_id: 87562,
                level_override: None,
            }, // ORE module → Mining 5
        ];
        let plan = build_skill_plan(&store, &targets, Some("en")).unwrap();

        let ids: Vec<u64> = plan.plan.iter().map(|s| s.skill_id).collect();
        assert_eq!(
            ids,
            vec![3386, 3410, 17940],
            "Mining → Astrogeology → Mining Barge"
        );

        let mining = &plan.plan[0];
        assert_eq!(
            mining.required_level, 5,
            "deduped to highest demanded level"
        );
        assert_eq!(
            mining.required_by,
            vec![17476, 87562],
            "provenance unions both targets"
        );
        assert_eq!(mining.sp_for_level, 256000);
        assert_eq!(plan.total_sp, 312000);
        assert_eq!(plan.plan.last().unwrap().cumulative_sp, 312000);

        // Covetor target tree roots at Mining Barge (its only direct prereq).
        let covetor = plan.targets.iter().find(|t| t.type_id == 17476).unwrap();
        assert_eq!(covetor.tree[0].skill_id, 17940);
        assert_eq!(covetor.tree[0].rank, 4);
    }

    #[test]
    fn build_skill_plan_errors_on_cycle() {
        // 100 requires 101, 101 requires 100 — both skills (have rank 275).
        let (_d, type_dogma) = make_index(
            "{\"_key\":100,\"dogmaAttributes\":[{\"attributeID\":275,\"value\":1.0},{\"attributeID\":182,\"value\":101.0},{\"attributeID\":277,\"value\":1.0}]}\n\
             {\"_key\":101,\"dogmaAttributes\":[{\"attributeID\":275,\"value\":1.0},{\"attributeID\":182,\"value\":100.0},{\"attributeID\":277,\"value\":1.0}]}\n",
        );
        let (_t, types) = make_index(
            "{\"_key\":100,\"name\":{\"en\":\"Loop A\"}}\n{\"_key\":101,\"name\":{\"en\":\"Loop B\"}}\n",
        );
        let store = Arc::new(SdeStore {
            type_dogma,
            types,
            ..default_store()
        });
        let targets = vec![SkillPlanTarget {
            type_id: 100,
            level_override: None,
        }];
        let err = build_skill_plan(&store, &targets, None).unwrap_err();
        assert!(err.contains("cycle"), "expected cycle error, got: {err}");
    }

    #[test]
    fn sp_breakdown_increments_sum_to_cumulative() {
        let b = sp_breakdown(1);
        assert_eq!(b[0].sp_to_reach, 250);
        assert_eq!(b[4].sp_to_reach, 256000);
        // increments are level-to-level deltas
        assert_eq!(b[0].increment, 250);
        assert_eq!(b[1].increment, 1414 - 250);
        // last increment + prior cumulative == final cumulative
        assert_eq!(b[3].sp_to_reach + b[4].increment, b[4].sp_to_reach);
    }

    #[test]
    fn skill_plan_steps_carry_full_sp_curve() {
        let store = fixture_store();
        let plan = build_skill_plan(
            &store,
            &[SkillPlanTarget {
                type_id: 87562,
                level_override: None,
            }],
            Some("en"),
        )
        .unwrap();
        // Mining (rank 1) demanded at L5 — its sp_by_level still spans all 5 levels.
        let mining = plan.plan.iter().find(|s| s.skill_id == 3386).unwrap();
        assert_eq!(mining.sp_by_level.len(), 5);
        assert_eq!(mining.sp_by_level[0].sp_to_reach, 250);
        assert_eq!(mining.sp_by_level[4].sp_to_reach, mining.sp_for_level);
    }

    #[tokio::test]
    async fn sde_get_modifiers_requires_exactly_one_arg() {
        let server = make_server();
        let err = server
            .sde_get_modifiers(Parameters(ModifierQueryParam {
                type_id: None,
                attribute_id: None,
                effect_id: None,
                levers: None,
                resolve_names: None,
            }))
            .await;
        assert!(err.is_err());
    }

    #[test]
    fn build_skill_plan_errors_when_depth_exceeded() {
        // Linear prereq chain 100→101→…→115 (16 deep), no cycle. Must trip the
        // depth guard (MAX_SKILL_DEPTH = 12), not the cycle guard.
        let mut dogma = String::new();
        for id in 100u64..=114 {
            dogma.push_str(&format!(
                "{{\"_key\":{id},\"dogmaAttributes\":[{{\"attributeID\":275,\"value\":1.0}},{{\"attributeID\":182,\"value\":{next}.0}},{{\"attributeID\":277,\"value\":1.0}}]}}\n",
                next = id + 1
            ));
        }
        // Leaf skill at the end of the chain (has rank, no further prereq).
        dogma
            .push_str("{\"_key\":115,\"dogmaAttributes\":[{\"attributeID\":275,\"value\":1.0}]}\n");
        let (_d, type_dogma) = make_index(&dogma);
        let store = Arc::new(SdeStore {
            type_dogma,
            ..default_store()
        });
        let targets = vec![SkillPlanTarget {
            type_id: 100,
            level_override: None,
        }];
        let err = build_skill_plan(&store, &targets, None).unwrap_err();
        assert!(err.contains("depth"), "expected depth error, got: {err}");
    }

    #[test]
    fn build_skill_plan_handles_empty_targets() {
        let store = fixture_store();
        let plan = build_skill_plan(&store, &[], Some("en")).unwrap();
        assert!(plan.plan.is_empty());
        assert!(plan.targets.is_empty());
        assert_eq!(plan.total_sp, 0);
    }

    #[test]
    fn build_skill_plan_flags_assumed_rank_for_rankless_skill() {
        // 200 is a module needing skill 201; 201 has no rank attribute (275), so
        // its rank is defaulted to 1 and the step must be flagged rank_assumed.
        let (_d, type_dogma) = make_index(
            "{\"_key\":200,\"dogmaAttributes\":[{\"attributeID\":182,\"value\":201.0},{\"attributeID\":277,\"value\":3.0}]}\n\
             {\"_key\":201,\"dogmaAttributes\":[]}\n",
        );
        let store = Arc::new(SdeStore {
            type_dogma,
            ..default_store()
        });
        let targets = vec![SkillPlanTarget {
            type_id: 200,
            level_override: None,
        }];
        let plan = build_skill_plan(&store, &targets, None).unwrap();
        let step = plan.plan.iter().find(|s| s.skill_id == 201).unwrap();
        assert!(step.rank_assumed, "rank-less skill should be flagged");
        assert_eq!(step.rank, 1, "defaults to rank 1");
    }

    #[test]
    fn operation_label_decodes_canonical_dogma_operators() {
        // op 6 is the one that bit the benchmark: percent, not flat.
        assert_eq!(
            operation_label(6),
            "postPercent (+magnitude% per stacking source)"
        );
        assert_eq!(operation_label(2), "modAdd (additive, flat)");
        assert_eq!(operation_label(4), "postMul (multiply)");
        assert_eq!(operation_label(0), "preMul (multiply)");
        assert_eq!(operation_label(7), "postAssignment (set, applied last)");
        assert_eq!(operation_label(123), "unknown");
    }

    #[test]
    fn levers_for_type_enumerates_all_attrs_with_skill_sources_first() {
        use crate::store::ModifierRef;
        // Module 100 has two attributes: 77 (miningAmount) and 5967 (miningCritChance).
        // Each is modified by one effect, owned respectively by Mining (a skill) and
        // Mining Precision (a skill). The levers view must surface BOTH attributes —
        // the crit attr is exactly what an agent anchoring on 77 would otherwise miss.
        // Module 100 requires skill 3386 (attr 182) — so modifiers gated on
        // skillTypeID 3386 apply; ones for other skills would be filtered out.
        let (_td, type_dogma) = make_index(
            "{\"_key\":100,\"dogmaAttributes\":[{\"attributeID\":182,\"value\":3386.0},{\"attributeID\":77,\"value\":200.0},{\"attributeID\":5967,\"value\":0.01}]}\n",
        );
        let (_ty, types) = make_index(
            "{\"_key\":3386,\"name\":{\"en\":\"Mining\"},\"groupID\":600}\n\
             {\"_key\":90727,\"name\":{\"en\":\"Mining Precision\"},\"groupID\":600}\n",
        );
        let (_gr, groups) = make_index("{\"_key\":600,\"categoryID\":16}\n");
        let mk = |effect_id, modified| ModifierRef {
            effect_id,
            modifying_attribute_id: 6049,
            modified_attribute_id: modified,
            operation: 6,
            func: None,
            domain: None,
            skill_type_id: Some(3386),
        };
        let attribute_modifiers =
            HashMap::from([(77u64, vec![mk(501, 77)]), (5967u64, vec![mk(500, 5967)])]);
        let effect_to_types = HashMap::from([(500u64, vec![90727u64]), (501u64, vec![3386u64])]);
        let store = Arc::new(SdeStore {
            type_dogma,
            types,
            groups,
            attribute_modifiers,
            effect_to_types,
            ..default_store()
        });
        let server = SdeMcpServer::new(store, None);
        let r = server.levers_for_type(100, false).unwrap();
        let attrs = r["attributes"].as_array().unwrap();
        // All three dogmaAttributes appear (incl. the requiredSkill1 meta-attr 182,
        // which has no modifiers) — completeness, no silent omission.
        assert_eq!(attrs.len(), 3, "every module attribute must appear");

        let crit = attrs
            .iter()
            .find(|a| a["attribute_id"] == 5967)
            .expect("crit attr present");
        assert_eq!(crit["modifier_count"], 1);
        let src = &crit["sources"][0];
        assert_eq!(src["type_id"], 90727); // Mining Precision surfaces as the lever
        assert_eq!(src["is_skill"], true);

        let yield_attr = attrs.iter().find(|a| a["attribute_id"] == 77).unwrap();
        assert_eq!(yield_attr["sources"][0]["type_id"], 3386);
        assert_eq!(yield_attr["sources"][0]["is_skill"], true);
    }

    #[tokio::test]
    async fn sde_get_modifiers_errors_on_unknown_attribute() {
        // Unknown attribute_id must error, not return a confident empty answer.
        let server = make_server();
        let err = server
            .sde_get_modifiers(Parameters(ModifierQueryParam {
                type_id: None,
                attribute_id: Some(999),
                effect_id: None,
                levers: None,
                resolve_names: None,
            }))
            .await;
        assert!(err.is_err());
        assert!(err.unwrap_err().message.contains("999"));
    }

    #[tokio::test]
    async fn sde_get_modifiers_returns_empty_for_unmodified_attribute() {
        // Attribute exists but nothing modifies it → empty list, not an error.
        let (_a, dogma_attributes) =
            make_index("{\"_key\":77,\"name\":{\"en\":\"miningAmount\"}}\n");
        let store = Arc::new(SdeStore {
            dogma_attributes,
            ..default_store()
        });
        let server = SdeMcpServer::new(store, None);
        let out = server
            .sde_get_modifiers(Parameters(ModifierQueryParam {
                type_id: None,
                attribute_id: Some(77),
                effect_id: None,
                levers: None,
                resolve_names: Some(false),
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["attribute_id"], 77);
        assert_eq!(v["modified_by"].as_array().unwrap().len(), 0);
    }
}
