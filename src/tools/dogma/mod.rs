//! Dogma: the DogmaAttributes and DogmaEffects that give a Type its stats, and
//! the modifiers that boost or penalise them.
//!
//! Owns the ExplicitValue-vs-DefaultValue discipline. An attribute a Type records
//! is an ExplicitValue; every other Type still HAS the attribute, at its
//! DefaultValue, and no tool here ever reports the second as the first. `types`
//! borrows the projection and annotation helpers rather than restating that rule.

use std::collections::{HashMap, HashSet};

use rmcp::{
    ErrorData,
    handler::server::wrapper::Parameters,
    schemars::{self, JsonSchema},
    tool, tool_router,
};
use serde::Deserialize;

use crate::tools::SdeMcpServer;
use crate::tools::query;
use crate::tools::query::{HasMatchedFields, matching_records};

// ── Parameter structs ────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
pub struct AttributeIdParam {
    pub attribute_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct EffectIdParam {
    pub effect_id: u64,
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
pub struct SearchDogmaParam {
    /// Substring to look for, matched case-insensitively against the name, display
    /// name and description of every DogmaAttribute and DogmaEffect. Spaced
    /// phrases work: "jump fatigue" finds jumpFatigueMultiplier through its display
    /// name even though the camelCase identifier holds no space.
    pub query: String,
    /// Maximum hits per list (default: 25, capped at 200). Attributes and effects
    /// are limited separately, so a term matching many attributes still shows its
    /// effects — the point of searching both in one call.
    pub limit: Option<u64>,
}

impl SdeMcpServer {
    /// In-place: annotate each dogmaAttribute with `attributeName`, and decode
    /// skill-prerequisite slots (182/183/184 + 277/278/279) into a `requiredSkill` object.
    pub(crate) fn annotate_dogma_names(&self, val: &mut serde_json::Value) {
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

    /// In-place: drop every `dogmaAttributes` entry the caller did not name. A
    /// requested attribute the Type records no ExplicitValue for has no entry to
    /// keep, so it is simply absent — the same distinction `sde_find_types`'
    /// attribute predicate enforces, and the reason this is not reported as a zero.
    ///
    /// `dogmaEffects` is left alone: this selects attributes (effect selection is
    /// out of scope), and dropping effects would make a projected response unusable
    /// for a caller that wants both. Projecting away attributes 277–279 also strips
    /// the levels [`Self::annotate_dogma_names`] pairs with a skill prerequisite, so
    /// a caller asking for 182 alone gets `level: 0` — its own projection, honoured.
    pub(crate) fn project_dogma_attributes(val: &mut serde_json::Value, wanted: &HashSet<u64>) {
        let Some(attrs) = val
            .get_mut("dogmaAttributes")
            .and_then(|a| a.as_array_mut())
        else {
            return;
        };
        attrs.retain(|a| {
            a.get("attributeID")
                .and_then(|x| x.as_u64())
                .is_some_and(|id| wanted.contains(&id))
        });
    }

    /// The DogmaAttribute IDs a caller named for projection, or `None` when it named
    /// none — an empty list is read as no projection, like every other empty list
    /// here. Shared by `sde_find_types`' `project_attributes` and
    /// `sde_get_types_dogma`' `attribute_ids` so the two cannot disagree about what
    /// a caller may ask for.
    ///
    /// Two different absences are deliberately not the same thing. An attribute the
    /// SDE declares but *this* Type records no ExplicitValue for is legitimate
    /// absence, and still yields no entry rather than a zero or the DefaultValue. An
    /// attribute the SDE declares **nowhere** is a typo, and answering a typo with a
    /// confidently empty projection is the failure this whole feature exists to end
    /// — so it is an error, naming the offending ID exactly as
    /// [`Self::resolve_group_filter`] does for an undeclared Group.
    pub(crate) fn resolve_attribute_projection(
        &self,
        ids: Option<&[u64]>,
    ) -> Result<Option<Vec<u32>>, ErrorData> {
        let Some(named) = ids.filter(|ids| !ids.is_empty()) else {
            return Ok(None);
        };
        named
            .iter()
            .map(|&id| {
                u32::try_from(id)
                    .ok()
                    .filter(|_| self.store.dogma_attributes.id_index.contains_key(&id))
                    .ok_or_else(|| {
                        ErrorData::invalid_params(
                            format!("attribute {id} not found in dogmaAttributes"),
                            None,
                        )
                    })
            })
            .collect::<Result<Vec<u32>, _>>()
            .map(Some)
    }

    /// `sde_search_dogma`: substring-match the scan-time text corpus, ordered and
    /// truncated per list. No file is read here — the corpus is resident, which is
    /// what lets all three fields of all ~6,300 records be searched per call.
    fn search_dogma(&self, p: &SearchDogmaParam) -> Result<SearchDogmaResult, ErrorData> {
        // An empty needle is inside every string, so it would answer with a capped
        // page of the whole corpus and no way to tell that from a real result.
        let query = p.query.trim();
        if query.is_empty() {
            return Err(ErrorData::invalid_params(
                "sde_search_dogma needs a non-empty query; an empty substring matches \
                 every DogmaAttribute and DogmaEffect",
                None,
            ));
        }
        let limit = p
            .limit
            .unwrap_or(DEFAULT_SEARCH_DOGMA_LIMIT)
            .min(MAX_SEARCH_DOGMA_LIMIT) as usize;

        let mut attributes: Vec<DogmaAttributeHit> = matching_records(
            &self.store.dogma_attribute_text,
            query,
            |record, matched_fields| DogmaAttributeHit {
                attribute_id: u64::from(record.id),
                name: record.name.clone(),
                display_name: record.display_name.clone(),
                description: record.description.clone(),
                matched_fields,
                default_value: record.default_value,
                // Always present, including as a zero: "nothing records this
                // attribute" is the answer that saves the caller a reverse lookup,
                // and it is the same index `sde_find_types` selects from, so the
                // two cannot disagree.
                explicit_type_count: self
                    .store
                    .attribute_types
                    .get(&record.id)
                    .map_or(0, |rows| rows.len() as u64),
            },
        );
        let mut effects: Vec<DogmaEffectHit> = matching_records(
            &self.store.dogma_effect_text,
            query,
            |record, matched_fields| DogmaEffectHit {
                effect_id: u64::from(record.id),
                name: record.name.clone(),
                display_name: record.display_name.clone(),
                description: record.description.clone(),
                matched_fields,
            },
        );

        let attributes_matched = attributes.len();
        let effects_matched = effects.len();
        attributes.truncate(limit);
        effects.truncate(limit);

        Ok(SearchDogmaResult {
            attributes_returned: attributes.len(),
            effects_returned: effects.len(),
            truncated: attributes_matched > attributes.len() || effects_matched > effects.len(),
            attributes,
            effects,
            attributes_matched,
            effects_matched,
        })
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
        // The wrong turn this whole feature exists to close. "Which Types MODIFY X"
        // and "which Types HAVE X" are near-identical in English and disjoint in the
        // data, so an empty modifier list reads as "the SDE knows nothing about this
        // attribute" when it means "nothing modifies it — Types may still carry it".
        // This fires at the moment of confusion rather than relying on the tool
        // description having been read first, which is why it is worth a response key.
        let nothing_modifies = rows.is_empty();
        let mut out = serde_json::json!({"attribute_id": attribute_id, "modified_by": rows});
        if nothing_modifies {
            let explicit_type_count = u32::try_from(attribute_id)
                .ok()
                .and_then(|id| self.store.attribute_types.get(&id))
                .map_or(0, Vec::len);
            out["explicit_type_count"] = serde_json::json!(explicit_type_count);
            out["guidance"] = serde_json::json!(format!(
                "Nothing in the SDE modifies attribute {attribute_id}, which is not the \
                 same as the SDE having no data for it: {explicit_type_count} Types \
                 record an ExplicitValue for it. MODIFIES and HAS are disjoint \
                 questions over disjoint data. To list the Types that carry it, call \
                 sde_find_types with attribute {{\"id\": {attribute_id}}}."
            ));
        }
        Ok(out)
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

#[tool_router(router = dogma_router, vis = "pub(crate)")]
impl SdeMcpServer {
    #[tool(
        description = "Resolve dogma modifiers with no prose parsing. Provide exactly one of: type_id (the attributes this skill/ship/module modifies), attribute_id (which skills/ships modify this attribute), or effect_id (the raw modifierInfo entries a dogma effect defines). Magnitudes come from the source type's dogmaAttributes. For attribute_id, each row gives source_type_id/source_type_name = the type that OWNS the effect (the actual bonus source, e.g. Astrogeology), one row per owning type; required_skill_id/required_skill_name is a separate required-skill FILTER on the boosted modules (e.g. Mining) — do NOT treat it as the source. operation_name decodes the operation int: 'postPercent' means magnitude is +x% PER stacking source (NOT a flat add), 'modAdd' is flat additive, 'postMul'/'preMul' multiply — read it before interpreting magnitude. To assess what affects a MODULE's effective output (yield/DPS/tank/etc.), call with type_id + levers:true FIRST: it lists every attribute on the module with a modifier_count and the distinct modifying sources (skills first), filtered to modifiers that actually apply to this module (by its required skills), so you enumerate ALL tunable levers (crit chance, duration, etc.) before deciding which matter — never infer the full picture from one 'obvious' attribute. Then drill into a specific attribute_id for full per-source rows. This tool answers which Types MODIFY an attribute; sde_find_types answers which Types HAVE one, as a stored ExplicitValue. They are disjoint questions over disjoint data, and the English is nearly identical — an empty modified_by here does NOT mean no Type carries the attribute, and when it is empty the response says how many do and points you at sde_find_types."
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

    #[tool(
        description = "Find DogmaAttributes and DogmaEffects by name — the first step of any dogma question, since every other dogma tool wants an ID. One call searches both, returned as separate `attributes` and `effects` lists, so you do not have to know in advance which one a term names. The query is a case-insensitive substring matched against each record's name, display name and description, and each hit reports which of those matched: attribute names are camelCase identifiers, so \"jump fatigue\" finds jumpFatigueMultiplier only through its display name \"Jump Fatigue Multiplier\" — searching the name alone would find nothing. Each attribute hit also carries default_value, the DefaultValue every Type without an ExplicitValue for it holds, and explicit_type_count, how many Types record an ExplicitValue for it — feed the attribute ID to sde_find_types to list them, or skip that call when the count is 0. Both lists are capped independently; attributes_matched / effects_matched report the true totals."
    )]
    async fn sde_search_dogma(
        &self,
        Parameters(p): Parameters<SearchDogmaParam>,
    ) -> Result<String, ErrorData> {
        let result = self.search_dogma(&p)?;
        Ok(serde_json::to_string(&result).unwrap())
    }

    #[tool(description = "Get a dogma effect by its effect ID")]
    async fn sde_get_dogma_effect(
        &self,
        Parameters(p): Parameters<EffectIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.dogma_effects, p.effect_id, "dogmaEffects")
    }
}

/// Per list, not per call: a term matching 40 attributes must not crowd its
/// effects out of the answer, or searching both in one call buys nothing.
const DEFAULT_SEARCH_DOGMA_LIMIT: u64 = 25;

/// Hard ceiling per list. Hits carry three text fields, so a page is ~10× the size
/// of a `sde_find_types` row; a caller asking for more is clamped rather than
/// refused, and `attributes_matched` / `effects_matched` still report the true
/// count so a narrower query is the obvious next move.
const MAX_SEARCH_DOGMA_LIMIT: u64 = 200;

/// One matched DogmaAttribute. `default_value` and `explicit_type_count` are what
/// make search → find a two-step: the first says how to read a Type's stored value,
/// the second whether a reverse lookup is worth making at all.
#[derive(Debug, serde::Serialize)]
pub(crate) struct DogmaAttributeHit {
    attribute_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// Which of `name`, `display_name` and `description` contained the query — the
    /// caller's own relevance signal, since a term found in a description alone is
    /// often incidental.
    matched_fields: Vec<&'static str>,
    /// The DogmaAttribute's DefaultValue: what every Type without an ExplicitValue
    /// for it holds. Omitted when the record declares none.
    #[serde(skip_serializing_if = "Option::is_none")]
    default_value: Option<f64>,
    /// How many Types record an ExplicitValue for this attribute — the size of the
    /// set `sde_find_types` would return for it. Zero is an answer, not an absence.
    explicit_type_count: u64,
}

/// One matched DogmaEffect. No DefaultValue and no ExplicitValue count: those are
/// properties of a DogmaAttribute, and a Type either carries an effect or does not.
#[derive(Debug, serde::Serialize)]
pub(crate) struct DogmaEffectHit {
    effect_id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    matched_fields: Vec<&'static str>,
}

/// The `sde_search_dogma` envelope. The two lists are counted and capped
/// separately, so `attributes_matched` against `attributes_returned` says whether
/// the attribute half of the answer is complete regardless of what the effect half
/// did.
#[derive(Debug, serde::Serialize)]
pub(crate) struct SearchDogmaResult {
    attributes: Vec<DogmaAttributeHit>,
    effects: Vec<DogmaEffectHit>,
    attributes_matched: usize,
    attributes_returned: usize,
    effects_matched: usize,
    effects_returned: usize,
    /// True when either list was cut. Explicit rather than left to be derived, so a
    /// capped page is never summarised as the complete set of matches.
    truncated: bool,
}

/// How an [`AttributePredicate`] narrows the Types holding an ExplicitValue.
/// Every variant is already restricted to those Types — `Exists` is that
/// restriction alone. `Ne` compares against a caller-supplied value; `NotDefault`
/// compares against the DogmaAttribute's own DefaultValue. They answer different
/// questions and both exist on purpose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttributeOp {
    Exists,
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    NotDefault,
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

impl HasMatchedFields for DogmaAttributeHit {
    fn matched_fields(&self) -> &[&'static str] {
        &self.matched_fields
    }
}

impl HasMatchedFields for DogmaEffectHit {
    fn matched_fields(&self) -> &[&'static str] {
        &self.matched_fields
    }
}

impl AttributeOp {
    pub(crate) fn parse(op: Option<&str>) -> Result<Self, ErrorData> {
        Ok(match op {
            None | Some("exists") => AttributeOp::Exists,
            Some("eq") => AttributeOp::Eq,
            Some("ne") => AttributeOp::Ne,
            Some("gt") => AttributeOp::Gt,
            Some("gte") => AttributeOp::Gte,
            Some("lt") => AttributeOp::Lt,
            Some("lte") => AttributeOp::Lte,
            Some("not_default") => AttributeOp::NotDefault,
            Some(other) => {
                return Err(ErrorData::invalid_params(
                    format!(
                        "unknown attribute op '{other}' (expected exists, eq, ne, gt, gte, lt, lte or not_default)"
                    ),
                    None,
                ));
            }
        })
    }

    /// The comparison operand, narrowed to the `f32` the SDE actually stores.
    /// Omitting `value` on a comparison operator is an error rather than a silent
    /// fallback to `exists`, which would answer a much broader question than asked.
    pub(crate) fn operand(self, value: Option<f64>) -> Result<Option<f32>, ErrorData> {
        match self {
            AttributeOp::Exists | AttributeOp::NotDefault => Ok(None),
            _ => value.map(|v| Some(v as f32)).ok_or_else(|| {
                ErrorData::invalid_params(
                    format!("attribute op '{}' requires a value", self.as_str()),
                    None,
                )
            }),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            AttributeOp::Exists => "exists",
            AttributeOp::Eq => "eq",
            AttributeOp::Ne => "ne",
            AttributeOp::Gt => "gt",
            AttributeOp::Gte => "gte",
            AttributeOp::Lt => "lt",
            AttributeOp::Lte => "lte",
            AttributeOp::NotDefault => "not_default",
        }
    }

    /// Does a stored ExplicitValue survive this operator? `default` is the
    /// DogmaAttribute's DefaultValue, needed only by `not_default`; an attribute
    /// with no declared default admits everything rather than dropping every row.
    pub(crate) fn admits(self, stored: f32, want: Option<f32>, default: Option<f64>) -> bool {
        let want = want.unwrap_or_default();
        match self {
            AttributeOp::Exists => true,
            AttributeOp::Eq => stored == want,
            AttributeOp::Ne => stored != want,
            AttributeOp::Gt => stored > want,
            AttributeOp::Gte => stored >= want,
            AttributeOp::Lt => stored < want,
            AttributeOp::Lte => stored <= want,
            AttributeOp::NotDefault => default.is_none_or(|d| stored != d as f32),
        }
    }
}

#[cfg(test)]
mod tests;
