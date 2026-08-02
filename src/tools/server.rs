use std::{
    collections::{BTreeMap, HashMap, HashSet},
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

use super::guidance::{
    NARROW_ONLY_PREDICATES, SERVER_INSTRUCTIONS, STANDALONE_PREDICATES, guidance_for,
};
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
    /// Restrict the search to these Groups. Applied before the limit, so a scoped
    /// search still returns a full page.
    pub group_ids: Option<Vec<u64>>,
    /// Restrict the search to these Categories, each resolving down through its
    /// Groups. ANDs with group_ids and with published_only.
    pub category_ids: Option<Vec<u64>>,
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
pub struct TypesDogmaParam {
    /// EVE type IDs to fetch in one call
    pub type_ids: Vec<u64>,
    /// Return only these DogmaAttributes for each Type instead of all of them — a
    /// field selector. A Type recording no ExplicitValue for one of them simply has
    /// no entry for it; that is absence, not a zero and not the DefaultValue. An
    /// attribute ID the SDE declares nowhere is a typo and is rejected. Effects are
    /// unaffected.
    pub attribute_ids: Option<Vec<u64>>,
    /// Join attributeID→name and decode skill-prerequisite attrs (182/183/184 +
    /// levels) into a `requiredSkill` object, exactly as sde_get_type_dogma does.
    /// Default false keeps the raw records.
    pub resolve_names: Option<bool>,
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

/// The ExplicitValue predicate on `sde_find_types`. Every operator is already
/// restricted to Types holding an ExplicitValue for `id`; the operator only
/// narrows further. See [`AttributeOp`].
#[derive(Deserialize, JsonSchema)]
pub struct AttributePredicate {
    /// DogmaAttribute ID to select on (e.g. 1971 jumpFatigueMultiplier)
    pub id: u64,
    /// One of "exists" (default), "eq", "ne", "gt", "gte", "lt", "lte",
    /// "not_default". The comparison operators require `value`.
    pub op: Option<String>,
    /// The value to compare each Type's ExplicitValue against. Required by every
    /// operator except "exists" and "not_default", which ignore it.
    pub value: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct FindTypesParam {
    /// Select Types by the DogmaAttribute values they record. Types sitting at the
    /// attribute's DefaultValue are never returned — see the response's
    /// `attribute_semantics`.
    pub attribute: Option<AttributePredicate>,
    /// Restrict to Types whose name contains this substring (case-insensitive).
    /// Narrows a candidate set; it cannot be the only predicate — use
    /// sde_search_types for a bare name search.
    pub query: Option<String>,
    /// Evaluate only these Types — "of the 60 I already hold, which carry an
    /// ExplicitValue for X", or "which are published". Unlike the other narrowing
    /// predicates this one can stand alone, because the set is yours and needs no
    /// scan to produce. IDs the SDE does not declare simply match nothing.
    pub type_ids: Option<Vec<u64>>,
    /// Restrict to Types belonging to any of these Groups (e.g. 898 Black Ops).
    pub group_ids: Option<Vec<u64>>,
    /// Restrict to Types whose Group belongs to any of these Categories (e.g. 6
    /// Ship). Combined with `group_ids` it narrows further: both must hold.
    pub category_ids: Option<Vec<u64>>,
    /// Restrict to Types whose MetaGroup is any of these (1 Tech I, 2 Tech II,
    /// 4 Faction, …). Most Types carry no MetaGroup at all; those are excluded and
    /// counted in the response's `excluded_no_meta_group` rather than being read as
    /// Tech I. Narrows a candidate set; it cannot be the only predicate.
    pub meta_group_ids: Option<Vec<u64>>,
    /// Drop unpublished Types. Applied before `limit`, so asking for N published
    /// Types returns N when N exist. Narrows a candidate set; it cannot be the
    /// only predicate.
    pub published_only: Option<bool>,
    /// Also return each row's ExplicitValue for these DogmaAttributes, in an
    /// `attributes` map. A projection, not a predicate: it never adds, drops or
    /// reorders a row. An attribute a Type records no ExplicitValue for is absent
    /// from its map — not a zero and not the attribute's DefaultValue. An attribute
    /// ID the SDE declares nowhere is a typo and is rejected.
    pub project_attributes: Option<Vec<u64>>,
    /// Maximum rows to return (default: 100, capped at 1000). Predicates apply to
    /// the whole candidate set first, so `total_matched` is the real count even
    /// when `truncated` is true.
    pub limit: Option<u64>,
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
        self.search_filtered_where(index, q, limit, |_| true)
    }

    /// As [`Self::search_filtered`], but keeping only the records whose `_key`
    /// satisfies `keep`. The predicate runs over the whole match set before the
    /// limit — see [`query::search_by_name`] — so a narrowed search still returns a
    /// full page when one exists.
    fn search_filtered_where(
        &self,
        index: &crate::store::SdeIndex,
        q: &str,
        limit: usize,
        keep: impl FnMut(u64) -> bool,
    ) -> Result<Vec<serde_json::Value>, ErrorData> {
        let mut results = query::search_by_name(index, q, limit, keep)
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
    fn project_dogma_attributes(val: &mut serde_json::Value, wanted: &HashSet<u64>) {
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

    /// `sde_find_types`: apply the predicates, sort, truncate, then hydrate only
    /// the surviving page. Kept off the `#[tool]` method so the growing predicate
    /// set stays testable as a plain function.
    fn find_types(&self, p: &FindTypesParam) -> Result<FindTypesResult, ErrorData> {
        let group_filter = self.resolve_group_filter(
            p.group_ids.as_deref().unwrap_or_default(),
            p.category_ids.as_deref().unwrap_or_default(),
        )?;
        let type_id_filter = resolve_type_id_filter(p);
        // Validated up front, so a typo'd attribute is rejected whether or not the
        // predicates happen to match anything.
        let projected = self.resolve_attribute_projection(p.project_attributes.as_deref())?;
        let mut attribute_default = None;

        // The candidate set comes from the narrowest index that any predicate
        // names — the attribute inverted index when there is an attribute
        // predicate, the Group index when the call is taxonomy-only. Each index is
        // stored ascending at scan time, so filtering preserves the order that
        // truncation then cuts.
        let mut matched: Candidates = match (p.attribute.as_ref(), &type_id_filter, &group_filter) {
            (Some(pred), _, _) => {
                let (rows, default_value) = self.attribute_candidates(pred)?;
                attribute_default = default_value;
                rows
            }
            // The caller handed the set over, so this is the narrowest posting list
            // there is and it costs no scan at all — which is why `type_ids`
            // produces where the other narrowing predicates cannot. An ID the SDE
            // does not declare is dropped rather than returned as a nameless,
            // groupless row; `total_matched` is what reports the shortfall.
            (None, Some(ids), _) => {
                let mut candidates: Candidates = ids
                    .iter()
                    .filter(|&&id| self.store.types.id_index.contains_key(&u64::from(id)))
                    .map(|&type_id| (type_id, None))
                    .collect();
                // Unlike every other candidate source this one is not an index run,
                // so it arrives in `HashSet` order and has to be sorted here or
                // `truncated` would cut a different page in every process.
                candidates.sort_unstable_by_key(|&(type_id, _)| type_id);
                candidates
            }
            (None, None, Some(groups)) => {
                let mut candidates: Candidates = groups
                    .iter()
                    .flat_map(|g| self.store.group_types.get(g).into_iter().flatten())
                    .map(|&type_id| (type_id, None))
                    .collect();
                // Each Group's run is already ascending from scan time, so one
                // Group needs no sort; a union of several interleaves them and does.
                // `sort → truncate` is the ADR's own order, and it is what makes
                // `truncated` honest about which rows were cut.
                if groups.len() > 1 {
                    candidates.sort_unstable_by_key(|&(type_id, _)| type_id);
                }
                candidates
            }
            (None, None, None) => {
                return Err(ErrorData::invalid_params(
                    format!(
                        "sde_find_types needs at least one predicate; available: \
                         {STANDALONE_PREDICATES}. {NARROW_ONLY_PREDICATES} narrow a \
                         candidate set but cannot produce one"
                    ),
                    None,
                ));
            }
        };

        // Every remaining predicate is answered from an in-memory index, so filtering
        // the *whole* match set costs a hash lookup per candidate rather than a
        // seek and parse. That is what lets `published_only` apply before the limit
        // — a request for N published Types returns N when N exist — and what lets
        // the rollup below count matches instead of returned rows.
        let published_only = p.published_only.unwrap_or(false);
        let meta_group_filter = resolve_meta_group_filter(p);
        let query_filter = self.resolve_query_filter(p.query.as_deref());
        let mut excluded_no_meta_group = 0u64;
        if group_filter.is_some()
            || published_only
            || meta_group_filter.is_some()
            || type_id_filter.is_some()
            || query_filter.is_some()
        {
            matched.retain(|&(type_id, _)| {
                let in_group = group_filter.as_ref().is_none_or(|groups| {
                    self.store
                        .type_group
                        .get(&type_id)
                        .is_some_and(|g| groups.contains(g))
                });
                let named = type_id_filter
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&type_id));
                // `name_index` is keyed by name and valued by type ID, so a name
                // substring resolves to IDs and the whole candidate set is filtered
                // without one seek.
                let name_matches = query_filter
                    .as_ref()
                    .is_none_or(|ids| ids.contains(&u64::from(type_id)));
                if !in_group
                    || !named
                    || !name_matches
                    || (published_only && !self.store.published_types.contains(&type_id))
                {
                    return false;
                }
                let Some(wanted) = meta_group_filter.as_ref() else {
                    return true;
                };
                // Judged last, and only on candidates every other predicate kept, so
                // the count answers "how many of the Types you were asking about
                // could not be classified" rather than inflating with rows that were
                // never in the running.
                match self.store.type_meta_group.get(&type_id) {
                    Some(meta_group) => wanted.contains(meta_group),
                    None => {
                        excluded_no_meta_group += 1;
                        false
                    }
                }
            });
        }

        let limit = p
            .limit
            .unwrap_or(DEFAULT_FIND_TYPES_LIMIT)
            .min(MAX_FIND_TYPES_LIMIT) as usize;
        let total_matched = matched.len();
        // How many Types record the attribute at all, before the operator and the
        // narrowing predicates cut the set. `total_matched` cannot answer that —
        // reading a post-filter zero as "nothing records this attribute" is what
        // told a caller asking for jumpFatigueMultiplier > 999 that the SDE has no
        // such attribute while 66 Types carry it.
        let attribute_recorded = p
            .attribute
            .as_ref()
            .and_then(|a| u32::try_from(a.id).ok())
            .and_then(|id| self.store.attribute_types.get(&id))
            .map_or(0, Vec::len);
        // `projected` was resolved and validated before any predicate ran, and is
        // applied inside `take(limit)` below so a truncated page never pays
        // projection for rows it is not returning.
        let types: Vec<FoundType> = matched
            .iter()
            .take(limit)
            .map(|&(type_id, value)| {
                let record = query::fetch_by_id(&self.store.types, type_id as u64).ok();
                FoundType {
                    type_id: type_id as u64,
                    // Deliberately a single-language string even when the server
                    // runs in all-languages mode (`--language` unset), unlike every
                    // other tool. A selector row exists to be scanned, and eight
                    // language variants cost ~5× per row for zero selection value;
                    // callers escalate to sde_get_types for the full record. Do not
                    // "fix" this to match the other tools.
                    name: record
                        .as_ref()
                        .and_then(|r| pick_name(r.get("name"), self.language.as_deref())),
                    group_id: record
                        .as_ref()
                        .and_then(|r| r.get("groupID").and_then(|g| g.as_u64())),
                    value,
                    attributes: projected
                        .as_deref()
                        .map(|wanted| self.project_explicit_values(type_id, wanted)),
                }
            })
            .collect();

        let types_len = types.len();
        Ok(FindTypesResult {
            returned: types_len,
            truncated: total_matched > types_len,
            types,
            total_matched,
            groups: self.roll_up_groups(&matched),
            // Reported whenever the filter ran, including as a zero: "none of your
            // candidates lacked a MetaGroup" is an answer, and its absence when the
            // filter is inactive keeps the key from reading as "none were dropped".
            excluded_no_meta_group: meta_group_filter
                .is_some()
                .then_some(excluded_no_meta_group),
            attribute_semantics: p
                .attribute
                .as_ref()
                .map(|_| ATTRIBUTE_SEMANTICS.to_string()),
            attribute_default,
            // The reciprocal of `sde_get_modifiers`' pointer, fired on the same
            // moment of confusion approached from the other side: an attribute
            // predicate that matched nothing, where the caller may have wanted
            // MODIFIES all along. Present only when an attribute predicate actually
            // ran and returned nothing — a zero-match taxonomy query has no such
            // ambiguity to resolve.
            guidance: guidance_for(
                p.attribute.as_ref().map(|a| a.id),
                // Counted off the inverted index, not off `matched`: the message
                // turns on whether the SDE records the attribute at all, which is
                // true or false before this call's operator and narrowing
                // predicates ever run.
                attribute_recorded,
                total_matched,
                total_matched > types_len,
                |attr| self.store.attribute_modifiers.contains_key(&attr),
            ),
        })
    }

    /// Count the Groups across the **full** match set — every Type the predicates
    /// admitted, not the page that survived `limit`. Deriving this from the
    /// returned rows would describe 100 of category 91's 11,836 Types as if they
    /// were the taxonomy, which is the whole reason the rollup exists.
    ///
    /// Costs one hash lookup per match plus one record read per *distinct* Group
    /// (391 at worst in build 3444265, for category 11).
    fn roll_up_groups(&self, matched: &Candidates) -> Vec<GroupRollup> {
        let mut counts: HashMap<Option<u32>, u64> = HashMap::new();
        for &(type_id, _) in matched {
            let group_id = self.store.type_group.get(&type_id).copied();
            *counts.entry(group_id).or_default() += 1;
        }

        let mut rollup: Vec<GroupRollup> = counts
            .into_iter()
            .map(|(group_id, count)| GroupRollup {
                group_id: group_id.map(u64::from),
                name: group_id.and_then(|g| {
                    let record = query::fetch_by_id(&self.store.groups, u64::from(g)).ok()?;
                    pick_name(record.get("name"), self.language.as_deref())
                }),
                count,
            })
            .collect();
        // Biggest Group first — the shape of a Category is the question this
        // answers — with the ID as a tiebreak so repeated calls agree.
        rollup.sort_unstable_by_key(|g| (std::cmp::Reverse(g.count), g.group_id));
        rollup
    }

    /// The ExplicitValues `project_attributes` asked for, read off the same inverted
    /// index the attribute predicate selects from — so a projection costs no file
    /// read and cannot disagree with the predicate that chose the row. An attribute
    /// the Type records no ExplicitValue for is absent from the map: never a zero,
    /// never the DogmaAttribute's DefaultValue.
    ///
    /// `BTreeMap` so a row's keys come out in the same order in every process, and
    /// `f32` for the same reason [`FoundType::value`] is — `1.92` must stay `1.92`.
    fn project_explicit_values(&self, type_id: u32, wanted: &[u32]) -> BTreeMap<u64, f32> {
        wanted
            .iter()
            .filter_map(|&attribute_id| {
                // `attribute_types` runs are sorted by type_id at scan time, which
                // is what makes the (type, attribute) random access a binary search
                // rather than a walk of an attribute's 5,921 rows.
                let rows = self.store.attribute_types.get(&attribute_id)?;
                let at = rows.binary_search_by_key(&type_id, |&(t, _)| t).ok()?;
                Some((u64::from(attribute_id), rows[at].1))
            })
            .collect()
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
    fn resolve_attribute_projection(
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

    /// The IDs of every Type whose name contains `query`, or `None` when the call
    /// names none. Matching a substring against 52,821 names is one pass over
    /// resident strings, and the caller then tests membership against the type ID
    /// it already holds — no seek at any point.
    ///
    /// An empty or whitespace-only query is read as no query rather than as a
    /// substring every name contains — the two narrow the same set, except that
    /// treating it as a real filter would additionally drop Types missing from
    /// `name_index`.
    ///
    /// A name shared by several Types resolves to all of them: `name_index` keys
    /// one entry per name and values it with every ID carrying it.
    fn resolve_query_filter(&self, query: Option<&str>) -> Option<HashSet<u64>> {
        let needle = query.map(str::trim).filter(|q| !q.is_empty())?;
        Some(
            self.store
                .types
                .name_index
                .ids_containing(needle)
                .into_iter()
                // A predicate, not a ranking: `sde_find_types` orders by ID, so
                // the exact-name flag `search_by_name` sorts on is dropped here.
                .map(|hit| hit.id)
                .collect(),
        )
    }

    /// The Groups a call is restricted to, or `None` when it names no taxonomy
    /// predicate. `group_ids` and `category_ids` AND like every other predicate, so
    /// both collapse into one Group set: a Category contributes its Groups, and
    /// naming both keeps only the Groups satisfying each. An undeclared ID is an
    /// error rather than an empty answer — a confidently empty result for a typo'd
    /// ID is the failure this tool exists to end.
    ///
    /// Shared by `sde_find_types`, where taxonomy *produces* the candidate set, and
    /// `sde_search_types`, where it only scopes one a name substring produced. Both
    /// read the same `group_ids`/`category_ids` the same way, so the resolution
    /// lives here rather than once per tool.
    fn resolve_group_filter(
        &self,
        named_groups: &[u64],
        named_categories: &[u64],
    ) -> Result<Option<HashSet<u32>>, ErrorData> {
        if named_groups.is_empty() && named_categories.is_empty() {
            return Ok(None);
        }

        let mut groups = HashSet::new();
        for &id in named_groups {
            let group_id = u32::try_from(id)
                .ok()
                .filter(|_| self.store.groups.id_index.contains_key(&id))
                .ok_or_else(|| {
                    ErrorData::invalid_params(format!("group {id} not found in groups"), None)
                })?;
            groups.insert(group_id);
        }

        let mut from_categories = HashSet::new();
        for &id in named_categories {
            let category_id = u32::try_from(id)
                .ok()
                .filter(|_| self.store.categories.id_index.contains_key(&id))
                .ok_or_else(|| {
                    ErrorData::invalid_params(
                        format!("category {id} not found in categories"),
                        None,
                    )
                })?;
            // A declared Category with no Groups is not an error, just an empty
            // answer — unlike an undeclared one, which is a caller mistake.
            from_categories.extend(
                self.store
                    .category_groups
                    .get(&category_id)
                    .into_iter()
                    .flatten()
                    .copied(),
            );
        }

        if named_groups.is_empty() {
            return Ok(Some(from_categories));
        }
        if !named_categories.is_empty() {
            groups.retain(|g| from_categories.contains(g));
        }
        Ok(Some(groups))
    }

    /// The `(type_id, ExplicitValue)` rows an attribute predicate admits, plus the
    /// DogmaAttribute's DefaultValue for the envelope to echo.
    fn attribute_candidates(
        &self,
        pred: &AttributePredicate,
    ) -> Result<(Candidates, Option<f64>), ErrorData> {
        let op = AttributeOp::parse(pred.op.as_deref())?;
        let want = op.operand(pred.value)?;

        let attribute_id = u32::try_from(pred.id).map_err(|_| {
            ErrorData::invalid_params(
                format!("attribute {} not found in dogmaAttributes", pred.id),
                None,
            )
        })?;
        // `defaultValue` is read straight off the DogmaAttribute record by O(1)
        // offset — the candidate set itself never touches a file.
        let default_value = query::fetch_by_id(&self.store.dogma_attributes, pred.id)
            .ok()
            .and_then(|v| v.get("defaultValue").and_then(|d| d.as_f64()));
        let rows = self.store.attribute_types.get(&attribute_id);
        if default_value.is_none() && rows.is_none() {
            return Err(ErrorData::invalid_params(
                format!("attribute {} not found in dogmaAttributes", pred.id),
                None,
            ));
        }

        // `attribute_types` is already sorted by type_id at scan time, so the
        // filtered run inherits that order and repeated calls agree.
        let matched = rows
            .map(|v| v.as_slice())
            .unwrap_or_default()
            .iter()
            .copied()
            .filter(|&(_, value)| op.admits(value, want, default_value))
            .map(|(type_id, value)| (type_id, Some(value)))
            .collect();
        Ok((matched, default_value))
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

    #[tool(
        description = "Search types by name substring, returning whole Type records. Optionally scope the search to group_ids and/or category_ids (a Category resolves down through its Groups) — searching 'raven' unscoped also returns its blueprint, its SKINs and every NPC variant sharing the word, and category_ids: [6] is how you keep the ship. published_only drops unpublished Types. Every filter applies to the whole match set before the limit, so a scoped search still returns a full page, and results come back in ascending type ID so the same question gets the same answer twice. Use sde_find_types instead when you want a set defined by an attribute or taxonomy rather than by a name, or minimal rows over whole records."
    )]
    async fn sde_search_types(
        &self,
        Parameters(p): Parameters<SearchTypesParam>,
    ) -> Result<String, ErrorData> {
        let limit = p.limit.unwrap_or(10) as usize;
        let published_only = p.published_only.unwrap_or(false);
        let group_filter = self.resolve_group_filter(
            p.group_ids.as_deref().unwrap_or_default(),
            p.category_ids.as_deref().unwrap_or_default(),
        )?;
        // Both predicates are answered from indexes the scan already built, so
        // filtering the whole match set costs a hash lookup per candidate rather
        // than a seek and parse. `published_types` is the same `published` field
        // this used to read off each returned record — read before the limit now,
        // which is the point.
        let results = self.search_filtered_where(&self.store.types, &p.query, limit, |id| {
            if !published_only && group_filter.is_none() {
                return true;
            }
            let Ok(type_id) = u32::try_from(id) else {
                return false;
            };
            if published_only && !self.store.published_types.contains(&type_id) {
                return false;
            }
            group_filter.as_ref().is_none_or(|groups| {
                self.store
                    .type_group
                    .get(&type_id)
                    .is_some_and(|g| groups.contains(g))
            })
        })?;
        Ok(serde_json::to_string(&results).unwrap())
    }

    #[tool(
        description = "Find the set of Types matching a predicate. Currently: attribute {id, op?, value?} selects Types that record an ExplicitValue for a DogmaAttribute, returned with that value; group_ids and category_ids select by taxonomy (a Category resolves down through its Groups); meta_group_ids narrows to a tier (1 Tech I, 2 Tech II, 4 Faction, ...); published_only drops unpublished Types; query narrows by name substring; type_ids evaluates only the Types you name, which is how you ask 'of these 60 I already hold, which are published / carry attribute X'. meta_group_ids, published_only and query narrow a candidate set and cannot be the only predicate; type_ids can, because you supplied the set. All predicates AND, so 'Black Ops hulls with a non-default jump fatigue multiplier' is one call. The attribute predicate is ExplicitValue-only — every Type technically HAS every attribute at its DefaultValue, so Types with no stored row are absent and the response says so. op is exists (default), eq, ne, gt, gte, lt, lte (each needs value), or not_default (drops Types whose stored value merely restates the attribute's DefaultValue). Every response carries a `groups` rollup naming each matched Group and its count, computed over the full match set rather than the returned page, so it stays correct when `truncated` is true — that rollup, not the rows, is how you ask what Groups a Category contains. project_attributes adds an `attributes` map of the DogmaAttribute values you name to every returned row, so 'the jump fatigue of every Black Ops hull' is one call and one page of ~70-byte rows; an attribute a Type records no ExplicitValue for is absent from its map rather than reported as zero or as the DefaultValue, while an attribute ID the SDE declares nowhere is rejected as a typo. Most Types carry no MetaGroup at all, so a meta_group_ids call also returns excluded_no_meta_group: the candidates dropped for having none. Absence is not Tech I — those Types are unclassified, not tier 1. Contrast sde_get_modifiers, which answers which Types MODIFY an attribute — a disjoint question with disjoint data; an empty answer there does not mean no Type carries the attribute."
    )]
    async fn sde_find_types(
        &self,
        Parameters(p): Parameters<FindTypesParam>,
    ) -> Result<String, ErrorData> {
        let result = self.find_types(&p)?;
        Ok(serde_json::to_string(&result).unwrap())
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
        description = "Batch-get the dogma of multiple types by ID in one call. Returns one entry per input ID in order; missing IDs are reported with found:false. Pass attribute_ids to project each Type's dogma down to just those DogmaAttributes — ships carry over a hundred, so reading two fields across 60 of them costs kilobytes instead of hundreds. An attribute a Type records no ExplicitValue for is absent from its entry rather than an error, while an attribute ID the SDE declares nowhere is rejected as a typo; effects are returned either way. resolve_names annotates each returned attribute with its name and decodes skill prerequisites, exactly as sde_get_type_dogma does. Omitting both returns today's full record unchanged."
    )]
    async fn sde_get_types_dogma(
        &self,
        Parameters(p): Parameters<TypesDogmaParam>,
    ) -> Result<String, ErrorData> {
        // Validated and built once for the batch rather than per Type. An attribute
        // the SDE declares nowhere is rejected here; one a given Type simply does
        // not record still yields no entry, which is a different thing.
        let wanted: Option<HashSet<u64>> = self
            .resolve_attribute_projection(p.attribute_ids.as_deref())?
            .map(|ids| ids.into_iter().map(u64::from).collect());
        let resolve_names = p.resolve_names.unwrap_or(false);
        let out: Vec<_> = p
            .type_ids
            .iter()
            .map(|&id| match query::fetch_by_id(&self.store.type_dogma, id) {
                Ok(mut v) => {
                    // Project before resolving names, so a projected call joins
                    // names for the attributes asked for rather than the hundred a
                    // ship carries.
                    if let Some(wanted) = wanted.as_ref() {
                        Self::project_dogma_attributes(&mut v, wanted);
                    }
                    if resolve_names {
                        self.annotate_dogma_names(&mut v);
                    }
                    self.filter(&mut v);
                    serde_json::json!({"type_id": id, "found": true, "dogma": v})
                }
                Err(_) => serde_json::json!({"type_id": id, "found": false}),
            })
            .collect();
        Ok(serde_json::to_string(&out).unwrap())
    }

    #[tool(
        description = "Bulk-resolve type IDs to names and/or exact (case-insensitive) names to type IDs in one call. Lightweight id↔name mapping — use sde_search_types for substring search and sde_get_types for full records. A name several Types share resolves to one of them: the lowest published ID, or the lowest of all when none is published."
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
            .map(|name| {
                // One ID per name is this tool's contract and stays that way —
                // 1,016 names in build 3444265 are shared, up to 229 Types deep,
                // and callers needing the whole group use sde_search_types.
                //
                // The lowest *published* ID, falling back to the lowest of all.
                // Ranking by ID alone answered "Angel Control Tower" with the
                // unpublished 3591 rather than the published 27539 — a legacy
                // record every follow-up call then reports as having no data.
                // Publication is the SDE's own statement of which duplicate is
                // the live one, so it outranks age; the ID tiebreak still makes
                // the answer identical on every call.
                let ids = self.store.types.name_index.ids_for(&name.to_lowercase());
                let chosen = ids
                    .iter()
                    .copied()
                    .find(|&id| {
                        u32::try_from(id).is_ok_and(|id| self.store.published_types.contains(&id))
                    })
                    .or_else(|| ids.first().copied());
                match chosen {
                    Some(id) => serde_json::json!({"name": name, "type_id": id, "found": true}),
                    None => serde_json::json!({"name": name, "found": false}),
                }
            })
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

// ── sde_find_types ───────────────────────────────────────────────────────────

const DEFAULT_FIND_TYPES_LIMIT: u64 = 100;

/// The working match set: a Type ID plus, when an attribute predicate supplied
/// one, the ExplicitValue it matched on. Held as `f32` end to end so `1.92` stays
/// `1.92` rather than becoming its f64 widening.
type Candidates = Vec<(u32, Option<f32>)>;

/// Hard ceiling on rows per call. A caller asking for more is clamped rather than
/// refused: `total_matched` still reports the true size and `truncated` still says
/// the page is partial, so the answer stays honest. Category 91 holds 11,836 Types
/// and no client wants them inline.
const MAX_FIND_TYPES_LIMIT: u64 = 1000;

/// Stated on every attribute-predicate response. The whole point of the tool is
/// that "no row" and "no value" are different, so the caller is told which one a
/// short result means rather than being left to guess.
const ATTRIBUTE_SEMANTICS: &str = "ExplicitValue only: rows are Types that record a value for this \
     DogmaAttribute in typeDogma. Every other Type still HAS the attribute, at \
     attribute_default, and is deliberately not listed.";

/// The MetaGroups a call is restricted to, or `None` when it names none. Unlike
/// [`SdeMcpServer::resolve_group_filter`] this validates nothing: `metaGroups.jsonl`
/// is deliberately not scanned, so the server holds no list of declared MetaGroups
/// to check an ID against and cannot tell a typo from a MetaGroup no Type uses. An
/// ID too large to be one is dropped rather than errored for the same reason — it
/// simply matches nothing, which is what the response then reports.
/// The Types a call is restricted to, or `None` when it names none. Doubles as a
/// candidate source: it is the one narrowing predicate that can stand alone,
/// because producing from it is bounded by what the caller typed rather than by a
/// scan — see ADR 0003's amendment.
///
/// Validates nothing, unlike the taxonomy predicates. A caller handing over a set
/// it already holds is asking which of *those* match, and it is holding IDs it got
/// from this server; an ID the SDE never declared matches nothing and the shortfall
/// shows up in `total_matched` against the length of the list the caller sent.
fn resolve_type_id_filter(p: &FindTypesParam) -> Option<HashSet<u32>> {
    let named = p.type_ids.as_deref().filter(|ids| !ids.is_empty())?;
    Some(
        named
            .iter()
            .filter_map(|&id| u32::try_from(id).ok())
            .collect(),
    )
}

fn resolve_meta_group_filter(p: &FindTypesParam) -> Option<HashSet<u32>> {
    let named = p.meta_group_ids.as_deref().filter(|ids| !ids.is_empty())?;
    Some(
        named
            .iter()
            .filter_map(|&id| u32::try_from(id).ok())
            .collect(),
    )
}

/// One row of a `sde_find_types` answer. `value` is `f32` so it serializes as the
/// value the SDE stores (`1.92`), not the f64 widening of it (`1.9199999570846558`).
#[derive(Debug, serde::Serialize)]
pub(crate) struct FoundType {
    type_id: u64,
    name: Option<String>,
    group_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<f32>,
    /// The ExplicitValues `project_attributes` asked for, present exactly when it
    /// was given — including as an empty map, which says this Type records none of
    /// them rather than that no projection ran. A requested attribute missing from
    /// the map is one the Type has no ExplicitValue for; it still HAS the attribute,
    /// at its DefaultValue, exactly as `attribute_semantics` describes.
    #[serde(skip_serializing_if = "Option::is_none")]
    attributes: Option<BTreeMap<u64, f32>>,
}

/// One Group in a `sde_find_types` rollup. `group_id` and `name` are optional for
/// the same reason `FoundType`'s are: a Type reachable through an index but absent
/// from `types.jsonl` still has to be counted, or the rollup would stop summing to
/// `total_matched`.
#[derive(Debug, serde::Serialize)]
pub(crate) struct GroupRollup {
    group_id: Option<u64>,
    name: Option<String>,
    count: u64,
}

/// The `sde_find_types` envelope. `total_matched` counts the whole match set, not
/// the returned page, so a truncated answer still reports the true size. `groups`
/// counts that same full match set — see [`SdeMcpServer::roll_up_groups`].
#[derive(Debug, serde::Serialize)]
pub(crate) struct FindTypesResult {
    types: Vec<FoundType>,
    total_matched: usize,
    returned: usize,
    truncated: bool,
    groups: Vec<GroupRollup>,
    /// Candidates dropped for carrying no MetaGroup at all, present only when
    /// `meta_group_ids` was given. Most Types have no MetaGroup, so a MetaGroup
    /// filter silently discards the majority; this is what stops a caller reading
    /// the survivors as the whole population.
    #[serde(skip_serializing_if = "Option::is_none")]
    excluded_no_meta_group: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attribute_semantics: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attribute_default: Option<f64>,
    /// Fires on the two responses a caller is most likely to misread: an attribute
    /// predicate that matched nothing (which may mean they wanted MODIFIES), and a
    /// truncated page (where the `groups` rollup is the narrowing axis but nothing
    /// says so). Absent otherwise, per the self-description rule — a guidance key
    /// present on every response would be noise the caller learns to skip, which
    /// defeats the point of it appearing at the moment of confusion.
    #[serde(skip_serializing_if = "Option::is_none")]
    guidance: Option<String>,
}

// ── sde_search_dogma ─────────────────────────────────────────────────────────

/// Per list, not per call: a term matching 40 attributes must not crowd its
/// effects out of the answer, or searching both in one call buys nothing.
const DEFAULT_SEARCH_DOGMA_LIMIT: u64 = 25;

/// Hard ceiling per list. Hits carry three text fields, so a page is ~10× the size
/// of a `sde_find_types` row; a caller asking for more is clamped rather than
/// refused, and `attributes_matched` / `effects_matched` still report the true
/// count so a narrower query is the obvious next move.
const MAX_SEARCH_DOGMA_LIMIT: u64 = 200;

/// The fields a `sde_search_dogma` hit can match on, in the order they are
/// reported. A record carrying none of the query's fields never becomes a hit; a
/// record missing a field simply cannot list it, which is how a hit on attribute
/// 277 (no `displayName` at all) reports `["description"]` and nothing else.
const DOGMA_TEXT_FIELDS: [&str; 3] = ["name", "display_name", "description"];

/// Every corpus record the query matches, built into `T` by `into_hit` and ordered
/// name-matches-first, then ascending by ID.
///
/// The tiers matter under truncation: a name hit is the identifier the caller is
/// looking for, while a description hit is often incidental, and burying the former
/// behind a lower-numbered instance of the latter is how a search gets read as
/// "not in the SDE". Within a tier the corpus's scan-time ID order survives,
/// because [`slice::sort_by_key`] is stable — so repeated calls, and calls in
/// different processes, agree.
fn matching_records<T>(
    corpus: &[crate::store::DogmaText],
    query: &str,
    mut into_hit: impl FnMut(&crate::store::DogmaText, Vec<&'static str>) -> T,
) -> Vec<T>
where
    T: HasMatchedFields,
{
    let mut hits: Vec<T> = corpus
        .iter()
        .filter_map(|record| {
            let fields = [
                record.name.as_deref(),
                record.display_name.as_deref(),
                record.description.as_deref(),
            ];
            let matched: Vec<&'static str> = DOGMA_TEXT_FIELDS
                .iter()
                .zip(fields)
                .filter(|(_, text)| text.is_some_and(|t| contains_ignore_case(t, query)))
                .map(|(field, _)| *field)
                .collect();
            (!matched.is_empty()).then(|| into_hit(record, matched))
        })
        .collect();

    hits.sort_by_key(|hit| u8::from(hit.matched_fields().first() != Some(&"name")));
    hits
}

/// Lets [`matching_records`] rank the two hit shapes without either of them
/// growing a sort key field that would then be serialized onto the wire.
trait HasMatchedFields {
    fn matched_fields(&self) -> &[&'static str];
}

/// ASCII case-insensitive substring test, allocating nothing. The corpus is
/// English, so ASCII folding is the whole job; lowercasing every field of every
/// record per query would allocate ~800 KB to answer one substring question.
fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    let (haystack, needle) = (haystack.as_bytes(), needle.as_bytes());
    haystack.len() >= needle.len()
        && haystack
            .windows(needle.len())
            .any(|window| window.eq_ignore_ascii_case(needle))
}

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
enum AttributeOp {
    Exists,
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    NotDefault,
}

impl AttributeOp {
    fn parse(op: Option<&str>) -> Result<Self, ErrorData> {
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
    fn operand(self, value: Option<f64>) -> Result<Option<f32>, ErrorData> {
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
    fn admits(self, stored: f32, want: Option<f32>, default: Option<f64>) -> bool {
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
    use crate::tools::testkit::{
        default_store, make_blueprint_index, make_index, make_server, write_fixture,
    };

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
                group_ids: None,
                category_ids: None,
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0]["_key"], 34);
    }

    #[tokio::test]
    async fn sde_search_types_published_only_filters_unpublished() {
        // Scanned rather than hand-built: `published_only` now reads
        // `published_types`, which the same pass over types.jsonl fills, so the two
        // cannot drift apart in the fixture the way a hand-written store could.
        let (_f, path) = write_fixture(
            "{\"_key\":34,\"groupID\":18,\"name\":{\"en\":\"Tritanium\"},\"published\":true}\n\
             {\"_key\":35,\"groupID\":18,\"name\":{\"en\":\"Tritan Scrap\"},\"published\":false}\n",
        );
        let scanned =
            crate::scan::scan_types_pub(&path, &indicatif::ProgressBar::hidden()).unwrap();
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                types: scanned.index,
                published_types: scanned.published_types,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_search_types(Parameters(SearchTypesParam {
                query: "tritan".to_string(),
                limit: None,
                published_only: Some(true),
                group_ids: None,
                category_ids: None,
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
        let seam = crate::tools::testkit::Seam::serving(make_server().store, None).await?;
        let tools = seam.client.list_all_tools().await?;
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
        seam.shutdown().await
    }

    /// The agent-visible contract: every tool's name, description and input schema,
    /// as a client sees them over `tools/list`. Descriptions are prompt-critical —
    /// they are what an agent routes on — so a reworded one is a behavioral change
    /// even though no code path moved, and a dropped router is invisible to every
    /// other test here. Regenerate deliberately with
    /// `SDE_UPDATE_TOOLS_LIST=1 cargo test tools_list_matches_the_pinned_contract`
    /// when a tool is genuinely added or changed.
    #[tokio::test]
    async fn tools_list_matches_the_pinned_contract() -> anyhow::Result<()> {
        let seam = crate::tools::testkit::Seam::serving(make_server().store, None).await?;
        let mut tools = seam.client.list_all_tools().await?;
        seam.shutdown().await?;

        // Sorted by name so the snapshot does not encode router composition order,
        // which is an implementation detail; `serde_json::Map` is a `BTreeMap` here,
        // so every nested key order is already canonical.
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        let actual = serde_json::to_string_pretty(&tools)? + "\n";

        let golden =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tools-list.json");
        if std::env::var_os("SDE_UPDATE_TOOLS_LIST").is_some() {
            std::fs::write(&golden, &actual)?;
            return Ok(());
        }
        let expected = std::fs::read_to_string(&golden)?;
        assert_eq!(
            actual,
            expected,
            "the MCP tool contract drifted from {}",
            golden.display()
        );
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
    async fn sde_get_solar_system_by_name_prefers_the_system_actually_named_that() {
        // Both real records: Mohas (30000031) sorts ahead of Moh (30000042) by ID and
        // contains its name, so the single row this tool takes used to be the wrong
        // system — silently, and then fed onward into sde_find_route as a `_key`.
        let (_f, map_solar_systems) = make_index(
            "{\"_key\":30000031,\"name\":{\"en\":\"Mohas\"}}\n{\"_key\":30000042,\"name\":{\"en\":\"Moh\"}}\n",
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
                name: Some("Moh".to_string()),
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 30000042);
        assert_eq!(v["name"]["en"], "Moh");
    }

    #[tokio::test]
    async fn sde_get_region_by_name_prefers_the_region_actually_named_that() {
        let (_f, map_regions) = make_index(
            "{\"_key\":10000001,\"name\":{\"en\":\"Derelik North\"}}\n{\"_key\":10000002,\"name\":{\"en\":\"Derelik\"}}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                map_regions,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_region(Parameters(RegionParam {
                region_id: None,
                name: Some("Derelik".to_string()),
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 10000002);
    }

    #[tokio::test]
    async fn sde_resolve_types_prefers_the_published_record_of_a_shared_name() {
        // Both real: 3591 and 27539 are both "Angel Control Tower", and the lower ID
        // is the unpublished legacy record. Answering with it sends every follow-up
        // call — get_type, get_type_dogma, get_blueprint_for_product — at a placeholder.
        let (_f, types) = make_index(
            "{\"_key\":3591,\"name\":{\"en\":\"Angel Control Tower\"},\"published\":false}\n\
             {\"_key\":27539,\"name\":{\"en\":\"Angel Control Tower\"},\"published\":true}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                types,
                published_types: HashSet::from([27539]),
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_resolve_types(Parameters(ResolveTypesParam {
                type_ids: None,
                names: Some(vec!["angel control tower".to_string()]),
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["by_name"][0]["type_id"], 27539);
        assert_eq!(v["by_name"][0]["found"], true);
    }

    #[tokio::test]
    async fn sde_resolve_types_falls_back_to_the_lowest_id_when_none_is_published() {
        // Publication only breaks the tie when the SDE states it. With no published
        // carrier the answer is still the lowest ID, and still the same on every call.
        let (_f, types) = make_index(
            "{\"_key\":10248,\"name\":{\"en\":\"Ghost\"},\"published\":false}\n\
             {\"_key\":10252,\"name\":{\"en\":\"Ghost\"},\"published\":false}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                types,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_resolve_types(Parameters(ResolveTypesParam {
                type_ids: None,
                names: Some(vec!["Ghost".to_string()]),
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["by_name"][0]["type_id"], 10248);
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

    /// The MCP seam: a real scan of `tests/fixtures/sde`, a real `SdeMcpServer`,
    /// and a real MCP client talking to it over an in-memory duplex transport.
    /// Every test here drives a tool the way a client does — over the wire, not
    /// by calling the handler method directly.
    mod mcp_seam {
        use crate::tools::testkit::{Seam, ids_of, keys_of};

        #[tokio::test]
        async fn status_reports_the_scanned_build() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam.call("sde_status", serde_json::json!({})).await?;
            assert_eq!(r["build"], 3333874);
            assert_eq!(r["release_date"], "2024-01-15");
            assert!(r["files_scanned"].as_u64().unwrap() > 0);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_type_returns_the_record_for_an_id() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_get_type", serde_json::json!({"type_id": 34}))
                .await?;
            assert_eq!(r["_key"], 34);
            assert_eq!(r["name"], "Tritanium");
            seam.shutdown().await
        }

        #[tokio::test]
        async fn search_types_matches_a_name_substring() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_search_types", serde_json::json!({"query": "trit"}))
                .await?;
            assert!(r.as_array().unwrap().iter().any(|v| v["_key"] == 34));
            seam.shutdown().await
        }

        #[tokio::test]
        async fn search_types_orders_by_id_across_separate_processes() -> anyhow::Result<()> {
            // Two independent boots, not two calls to one server: `HashMap`
            // iteration order is fixed for the life of a process and reseeded
            // between them, so a loop against a single `Seam` would pass on the
            // nondeterministic ordering this replaced.
            //
            // The Types named exactly "mining" lead — only the skill 3386 is — and
            // the rest follow by ID. Both halves are ordered by the record, never by
            // arrival, so the two boots agree.
            let first = Seam::boot().await?;
            let a = first
                .call("sde_search_types", serde_json::json!({"query": "mining"}))
                .await?;
            first.shutdown().await?;

            let second = Seam::boot().await?;
            let b = second
                .call("sde_search_types", serde_json::json!({"query": "mining"}))
                .await?;
            second.shutdown().await?;

            assert_eq!(keys_of(&a), vec![3386, 1202, 3218, 10248, 10252, 17940]);
            assert_eq!(keys_of(&a), keys_of(&b));
            Ok(())
        }

        #[tokio::test]
        async fn search_types_returns_every_type_sharing_a_name() -> anyhow::Result<()> {
            // Types 36333 and 60106 are both named "Badger Wiyrkomi SKIN" in the
            // SDE. One ID per name kept whichever the scan reached last and dropped
            // the other — 2,228 Types in build 3444265, with no truncation flag and
            // no warning to say so.
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_search_types", serde_json::json!({"query": "wiyrkomi"}))
                .await?;
            assert_eq!(keys_of(&r), vec![36333, 60106]);

            // And the shared name does not crowd out the rest of the match set:
            // "badger" is the Hauler plus both of its SKINs, in ID order.
            let badger = seam
                .call("sde_search_types", serde_json::json!({"query": "badger"}))
                .await?;
            assert_eq!(keys_of(&badger), vec![648, 36333, 60106]);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn search_types_scopes_to_groups_and_categories() -> anyhow::Result<()> {
            // "badger" unscoped is the Hauler and both of its SKINs — the exact
            // shape of the complaint: a fuzzy name search returns the SKINs and NPC
            // variants the caller then has to discard.
            let seam = Seam::boot().await?;
            let unscoped = seam
                .call("sde_search_types", serde_json::json!({"query": "badger"}))
                .await?;
            assert_eq!(keys_of(&unscoped), vec![648, 36333, 60106]);

            // Group 28 Hauler keeps the ship.
            let by_group = seam
                .call(
                    "sde_search_types",
                    serde_json::json!({"query": "badger", "group_ids": [28]}),
                )
                .await?;
            assert_eq!(keys_of(&by_group), vec![648]);

            // Category 91 SKINs keeps its complement, resolved down through group
            // 1950 — a Category owns no Types directly.
            let by_category = seam
                .call(
                    "sde_search_types",
                    serde_json::json!({"query": "badger", "category_ids": [91]}),
                )
                .await?;
            assert_eq!(keys_of(&by_category), vec![36333, 60106]);

            // Several Groups at once, and the two filters AND rather than union:
            // group 1950 is a SKIN, so naming it alongside category 6 Ship leaves
            // nothing.
            let several = seam
                .call(
                    "sde_search_types",
                    serde_json::json!({"query": "badger", "group_ids": [28, 1950]}),
                )
                .await?;
            assert_eq!(keys_of(&several), vec![648, 36333, 60106]);
            let anded = seam
                .call(
                    "sde_search_types",
                    serde_json::json!({"query": "badger", "group_ids": [1950], "category_ids": [6]}),
                )
                .await?;
            assert!(keys_of(&anded).is_empty());

            // An unscoped search is unchanged, and naming no Group and no Category
            // is not a filter that matches nothing.
            let empty_scope = seam
                .call(
                    "sde_search_types",
                    serde_json::json!({"query": "badger", "group_ids": [], "category_ids": []}),
                )
                .await?;
            assert_eq!(keys_of(&empty_scope), keys_of(&unscoped));
            seam.shutdown().await
        }

        #[tokio::test]
        async fn search_types_scope_applies_before_the_limit() -> anyhow::Result<()> {
            // Six Types match "mining"; four are drones. Scoping after the cap would
            // take some two of the six and then drop whatever was not a drone.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_search_types",
                    serde_json::json!({"query": "mining", "category_ids": [18], "limit": 2}),
                )
                .await?;
            assert_eq!(keys_of(&r), vec![1202, 3218]);

            // And it composes with published_only: of the four drones matching
            // "mining", 10248 and 10252 are unpublished, so a published page of two
            // is what exists — and a page of two is what comes back.
            let published = seam
                .call(
                    "sde_search_types",
                    serde_json::json!({
                        "query": "mining",
                        "category_ids": [18],
                        "published_only": true,
                        "limit": 2
                    }),
                )
                .await?;
            assert_eq!(keys_of(&published), vec![1202, 3218]);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn search_types_rejects_an_undeclared_group_or_category() -> anyhow::Result<()> {
            // Same contract as sde_find_types: a typo'd taxonomy ID is a caller
            // mistake, and answering it with an empty result set is the confident
            // wrong answer this epic exists to stop.
            let seam = Seam::boot().await?;
            let bad_group = seam
                .try_call(
                    "sde_search_types",
                    serde_json::json!({"query": "badger", "group_ids": [999999]}),
                )
                .await;
            assert!(bad_group.is_err(), "undeclared group must be an error");

            let bad_category = seam
                .try_call(
                    "sde_search_types",
                    serde_json::json!({"query": "badger", "category_ids": [999999]}),
                )
                .await;
            assert!(
                bad_category.is_err(),
                "undeclared category must be an error"
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn search_types_published_only_fills_the_page() -> anyhow::Result<()> {
            // Six Types match "mining" and four of them are published, so a page of
            // four is available. Filtering after the cap would take some four of the
            // six and then drop the unpublished ones, returning two or three.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_search_types",
                    serde_json::json!({"query": "mining", "limit": 4, "published_only": true}),
                )
                .await?;
            // Exactly-named first (the Mining skill), then the rest by ID.
            assert_eq!(keys_of(&r), vec![3386, 1202, 3218, 17940]);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_group_returns_the_record_for_an_id() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_get_group", serde_json::json!({"group_id": 18}))
                .await?;
            assert_eq!(r["_key"], 18);
            assert_eq!(r["name"], "Mineral");
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_category_returns_the_record_for_an_id() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_get_category", serde_json::json!({"category_id": 4}))
                .await?;
            assert_eq!(r["_key"], 4);
            assert_eq!(r["name"], "Material");
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_type_materials_returns_the_reprocessing_list() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_get_type_materials",
                    serde_json::json!({"type_id": 1230}),
                )
                .await?;
            assert_eq!(r["_key"], 1230);
            assert!(r["materials"].as_array().is_some());
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_type_dogma_returns_attributes_for_a_type() -> anyhow::Result<()> {
            // Ferox has dogma attributes in the fixture.
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_get_type_dogma", serde_json::json!({"type_id": 16227}))
                .await?;
            assert_eq!(r["_key"], 16227);
            assert!(r["dogmaAttributes"].as_array().is_some());
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_type_dogma_resolve_names_decodes_skill_prereqs() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_get_type_dogma",
                    serde_json::json!({"type_id": 17940, "resolve_names": true}),
                )
                .await?;
            let attrs = r["dogmaAttributes"].as_array().unwrap();
            assert!(
                attrs
                    .iter()
                    .any(|a| a["requiredSkill"]["skill_name"] == "Astrogeology"
                        && a["requiredSkill"]["level"] == 3)
            );
            seam.shutdown().await
        }

        /// The `attributeID`s of one `sde_get_types_dogma` entry, in returned order.
        fn attribute_ids_of(entry: &serde_json::Value) -> Vec<u64> {
            entry["dogma"]["dogmaAttributes"]
                .as_array()
                .expect("dogmaAttributes array")
                .iter()
                .map(|a| a["attributeID"].as_u64().expect("attributeID"))
                .collect()
        }

        #[tokio::test]
        async fn get_types_dogma_projects_to_the_named_attributes() -> anyhow::Result<()> {
            // The five haulers and Black Ops each store six attributes; a caller
            // comparing jump fatigue wants one of them.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_get_types_dogma",
                    serde_json::json!({
                        "type_ids": [648, 651, 22428], "attribute_ids": [1971]
                    }),
                )
                .await?;
            let entries = r.as_array().unwrap();
            assert_eq!(entries.len(), 3);
            for entry in entries {
                assert_eq!(entry["found"], true);
                assert_eq!(attribute_ids_of(entry), vec![1971]);
            }
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_types_dogma_projection_omits_an_attribute_the_type_does_not_carry()
        -> anyhow::Result<()> {
            // The Hobgoblin II records damageMultiplier (64) but no miningAmount
            // (77). Naming both must yield the one it has and no entry — not a zero
            // and not the DefaultValue — for the one it does not, and must not fail
            // the call.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_get_types_dogma",
                    serde_json::json!({"type_ids": [2456], "attribute_ids": [64, 77]}),
                )
                .await?;
            assert_eq!(r[0]["found"], true);
            assert_eq!(attribute_ids_of(&r[0]), vec![64]);

            // An attribute no fixture Type carries at all is the same answer.
            let none = seam
                .call(
                    "sde_get_types_dogma",
                    serde_json::json!({"type_ids": [2456], "attribute_ids": [1971]}),
                )
                .await?;
            assert_eq!(none[0]["found"], true);
            assert_eq!(attribute_ids_of(&none[0]), Vec::<u64>::new());
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_types_dogma_rejects_an_attribute_the_sde_does_not_declare()
        -> anyhow::Result<()> {
            // The other half of the pair above. An attribute a Type does not carry
            // is legitimate absence; an attribute that exists nowhere is a typo, and
            // answering a typo with a confidently empty projection is exactly the
            // failure this feature exists to end.
            let seam = Seam::boot().await?;
            let r = seam
                .try_call(
                    "sde_get_types_dogma",
                    serde_json::json!({"type_ids": [2456], "attribute_ids": [64, 999999]}),
                )
                .await;
            assert!(r.is_err(), "an undeclared attribute must be rejected");
            assert!(format!("{}", r.unwrap_err()).contains("999999"));
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_types_dogma_reads_an_empty_attribute_list_as_no_projection()
        -> anyhow::Result<()> {
            // Naming nothing is not the same as projecting everything away — it is
            // the same as not asking, which keeps the record byte-identical and
            // matches how sde_find_types reads an empty project_attributes.
            let seam = Seam::boot().await?;
            let empty = seam
                .call(
                    "sde_get_types_dogma",
                    serde_json::json!({"type_ids": [17940], "attribute_ids": []}),
                )
                .await?;
            let omitted = seam
                .call(
                    "sde_get_types_dogma",
                    serde_json::json!({"type_ids": [17940]}),
                )
                .await?;
            assert_eq!(empty, omitted);
            assert_eq!(attribute_ids_of(&empty[0]).len(), 5);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_types_dogma_resolve_names_annotates_the_batch() -> anyhow::Result<()> {
            // The parameter the single-Type call already takes; passing it here used
            // to be a schema error.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_get_types_dogma",
                    serde_json::json!({
                        "type_ids": [17940], "attribute_ids": [182, 278], "resolve_names": true
                    }),
                )
                .await?;
            let attrs = r[0]["dogma"]["dogmaAttributes"].as_array().unwrap();
            assert!(
                attrs.iter().any(|a| a["attributeName"] == "requiredSkill1"
                    && a["requiredSkill"]["skill_name"] == "Astrogeology"),
                "projected attributes carry their names: {r}"
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_types_dogma_without_the_new_parameters_returns_the_whole_record()
        -> anyhow::Result<()> {
            // Existing callers depend on this: omitting both parameters must leave
            // the record exactly as the single-Type call returns it.
            let seam = Seam::boot().await?;
            let batch = seam
                .call(
                    "sde_get_types_dogma",
                    serde_json::json!({"type_ids": [17940]}),
                )
                .await?;
            let single = seam
                .call("sde_get_type_dogma", serde_json::json!({"type_id": 17940}))
                .await?;
            assert_eq!(batch[0]["dogma"], single);
            assert_eq!(
                serde_json::to_string(&batch[0]["dogma"]).unwrap(),
                serde_json::to_string(&single).unwrap(),
                "byte-identical, not merely equal"
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_types_dogma_still_reports_missing_ids_per_id_under_projection()
        -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_get_types_dogma",
                    serde_json::json!({
                        "type_ids": [648, 999999, 22428], "attribute_ids": [1971]
                    }),
                )
                .await?;
            assert_eq!(r[0]["type_id"], 648);
            assert_eq!(r[0]["found"], true);
            assert_eq!(r[1]["type_id"], 999999);
            assert_eq!(r[1]["found"], false);
            assert_eq!(r[2]["found"], true);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_types_dogma_projection_shrinks_the_response() -> anyhow::Result<()> {
            // The point of the parameter. Fixture Types carry six attributes where a
            // real ship carries over a hundred, and the per-entry envelope is fixed
            // cost either way, so the ratio here badly understates the real one —
            // less than half is what six attributes can show.
            let seam = Seam::boot().await?;
            let types = serde_json::json!([648, 651, 22428, 22430, 85236]);
            let full = seam
                .call(
                    "sde_get_types_dogma",
                    serde_json::json!({"type_ids": types}),
                )
                .await?;
            let projected = seam
                .call(
                    "sde_get_types_dogma",
                    serde_json::json!({"type_ids": types, "attribute_ids": [1971]}),
                )
                .await?;
            let (full, projected) = (full.to_string().len(), projected.to_string().len());
            assert!(
                projected * 2 < full,
                "projected {projected} B vs full {full} B"
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_blueprint_returns_its_activities() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_get_blueprint",
                    serde_json::json!({"blueprint_type_id": 16228}),
                )
                .await?;
            assert_eq!(r["_key"], 16228);
            assert!(r["activities"]["manufacturing"].is_object());
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_blueprint_for_product_walks_the_reverse_map() -> anyhow::Result<()> {
            // The Ferox blueprint makes the Ferox.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_get_blueprint_for_product",
                    serde_json::json!({"product_type_id": 16227}),
                )
                .await?;
            assert_eq!(r["blueprint"]["_key"], 16228);
            assert_eq!(r["activity"], "manufacturing");
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_solar_system_returns_the_record_for_an_id() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_get_solar_system",
                    serde_json::json!({"system_id": 30000142}),
                )
                .await?;
            assert_eq!(r["_key"], 30000142);
            assert_eq!(r["name"], "Jita");
            seam.shutdown().await
        }

        #[tokio::test]
        async fn search_solar_systems_matches_a_name_substring() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_search_solar_systems",
                    serde_json::json!({"query": "jita"}),
                )
                .await?;
            assert!(r.as_array().unwrap().iter().any(|v| v["_key"] == 30000142));
            seam.shutdown().await
        }

        #[tokio::test]
        async fn search_solar_systems_orders_by_id_across_separate_processes() -> anyhow::Result<()>
        {
            // The SolarSystem search shares the Type search's helper and inherits
            // the same correction. The fixture's records are deliberately not in ID
            // order in the file, so scan order and ID order disagree.
            let first = Seam::boot().await?;
            let a = first
                .call(
                    "sde_search_solar_systems",
                    serde_json::json!({"query": "i"}),
                )
                .await?;
            first.shutdown().await?;

            let second = Seam::boot().await?;
            let b = second
                .call(
                    "sde_search_solar_systems",
                    serde_json::json!({"query": "i"}),
                )
                .await?;
            second.shutdown().await?;

            assert_eq!(
                keys_of(&a),
                vec![30000138, 30000140, 30000142, 30000144, 30000145, 30000149]
            );
            assert_eq!(keys_of(&a), keys_of(&b));
            Ok(())
        }

        #[tokio::test]
        async fn get_region_returns_the_record_for_an_id() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_get_region", serde_json::json!({"region_id": 10000002}))
                .await?;
            assert_eq!(r["_key"], 10000002);
            assert_eq!(r["name"], "The Forge");
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_constellation_returns_the_record_for_an_id() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_get_constellation",
                    serde_json::json!({"constellation_id": 20000020}),
                )
                .await?;
            assert_eq!(r["_key"], 20000020);
            assert_eq!(r["name"], "Kimotoro");
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_npc_station_returns_the_record_for_an_id() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_get_npc_station",
                    serde_json::json!({"station_id": 60003760}),
                )
                .await?;
            assert_eq!(r["_key"], 60003760);
            assert_eq!(r["solarSystemID"], 30000142);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_route_returns_the_shortest_stargate_path() -> anyhow::Result<()> {
            // Jita → Perimeter is 1 jump.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_route",
                    serde_json::json!({
                        "from_system_id": 30000142,
                        "to_system_id": 30000144,
                    }),
                )
                .await?;
            assert_eq!(r["jumps"], 1);
            assert_eq!(r["path"].as_array().unwrap().len(), 2);
            assert_eq!(r["path"][0], 30000142);
            assert_eq!(r["path"][1], 30000144);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_route_errors_when_no_path_exists() -> anyhow::Result<()> {
            // Ikuchi has no stargates in the fixture, so it is unreachable.
            let seam = Seam::boot().await?;
            let err = seam
                .try_call(
                    "sde_find_route",
                    serde_json::json!({
                        "from_system_id": 30000142,
                        "to_system_id": 30000138,
                    }),
                )
                .await;
            assert!(err.is_err(), "expected error for unreachable system");
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_market_group_returns_the_record_for_an_id() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_get_market_group",
                    serde_json::json!({"market_group_id": 1857}),
                )
                .await?;
            assert_eq!(r["_key"], 1857);
            assert_eq!(r["name"], "Minerals");
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_market_group_tree_returns_root_to_leaf_ancestry() -> anyhow::Result<()> {
            // Minerals → Materials → Manufacture & Research.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_get_market_group_tree",
                    serde_json::json!({"market_group_id": 1857}),
                )
                .await?;
            let arr = r.as_array().unwrap();
            assert_eq!(arr.len(), 3);
            assert_eq!(arr[0]["_key"], 475); // root: Manufacture & Research
            assert_eq!(arr[2]["_key"], 1857); // leaf: Minerals
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_dogma_attribute_returns_the_record_for_an_id() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_get_dogma_attribute",
                    serde_json::json!({"attribute_id": 30}),
                )
                .await?;
            assert_eq!(r["_key"], 30);
            assert_eq!(r["name"], "power");
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_dogma_effect_returns_the_record_for_an_id() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_get_dogma_effect", serde_json::json!({"effect_id": 11}))
                .await?;
            assert_eq!(r["_key"], 11);
            assert_eq!(r["name"], "loPower");
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_faction_returns_the_record_for_an_id() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_get_faction", serde_json::json!({"faction_id": 500001}))
                .await?;
            assert_eq!(r["_key"], 500001);
            assert_eq!(r["name"], "Caldari State");
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_npc_corporation_returns_the_record_for_an_id() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_get_npc_corporation",
                    serde_json::json!({"corporation_id": 1000035}),
                )
                .await?;
            assert_eq!(r["_key"], 1000035);
            assert_eq!(r["name"], "Caldari Navy");
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_skin_returns_the_record_for_an_id() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_get_skin", serde_json::json!({"skin_id": 50}))
                .await?;
            assert_eq!(r["_key"], 50);
            assert_eq!(r["internalName"], "Ferox Caldari Union Day YC124");
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_skill_plan_merges_multiple_targets_into_one_plan() -> anyhow::Result<()> {
            // Covetor + ORE Deep Core Strip Miner → one merged plan.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_get_skill_plan",
                    serde_json::json!({
                        "targets": [{"type_id": 17476}, {"type_id": 87562}]
                    }),
                )
                .await?;
            let plan = r["plan"].as_array().unwrap();
            // Mining deduped to level 5 (module demands 5), prereqs before dependents.
            assert_eq!(plan[0]["skill_id"], 3386);
            assert_eq!(plan[0]["required_level"], 5);
            assert_eq!(plan[0]["sp_for_level"], 256000);
            assert_eq!(plan[0]["required_by"].as_array().unwrap().len(), 2);
            assert_eq!(plan.last().unwrap()["skill_id"], 17940); // Mining Barge last
            assert_eq!(r["total_sp"], 312000);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_modifiers_by_attribute_lists_every_owning_type() -> anyhow::Result<()> {
            // Direction-b: what modifies miningAmount (77).
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_get_modifiers", serde_json::json!({"attribute_id": 77}))
                .await?;
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
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_modifiers_by_type_lists_outgoing_modifiers() -> anyhow::Result<()> {
            // Direction-a: the Mining skill's outgoing modifiers.
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_get_modifiers", serde_json::json!({"type_id": 3386}))
                .await?;
            assert!(
                r["modifies"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|m| m["modified_attribute_id"] == 77)
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_modifiers_by_effect_returns_raw_modifier_info() -> anyhow::Result<()> {
            // Direction-c: a dogma effect's raw modifierInfo.
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_get_modifiers", serde_json::json!({"effect_id": 391}))
                .await?;
            let m = &r["modifiers"][0];
            assert_eq!(m["modified_attribute_id"], 77);
            assert_eq!(m["modifying_attribute_id"], 434);
            assert_eq!(m["skill_type_id"], 3386);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_types_batch_flags_missing_ids() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_get_types",
                    serde_json::json!({"type_ids": [34, 999999]}),
                )
                .await?;
            let arr = r.as_array().unwrap();
            assert_eq!(arr[0]["found"], true);
            assert_eq!(arr[0]["type"]["name"], "Tritanium");
            assert_eq!(arr[1]["found"], false);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn resolve_types_maps_ids_and_names_in_both_directions() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_resolve_types",
                    serde_json::json!({"type_ids": [87562], "names": ["Covetor", "Mining"]}),
                )
                .await?;
            assert_eq!(r["by_id"][0]["name"], "ORE Deep Core Strip Miner");
            assert_eq!(r["by_name"][0]["type_id"], 17476);
            assert_eq!(r["by_name"][1]["type_id"], 3386);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_skill_sp_returns_the_full_curve_for_a_skill() -> anyhow::Result<()> {
            // Astrogeology is rank 3.
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_get_skill_sp", serde_json::json!({"type_id": 3410}))
                .await?;
            assert_eq!(r["rank"], 3);
            let lvls = r["levels"].as_array().unwrap();
            assert_eq!(lvls[0]["sp_to_reach"], 750); // rank3 L1 = 3 × 250
            assert_eq!(lvls[4]["sp_to_reach"], 768000); // rank3 L5 = 3 × 256000
            assert_eq!(lvls[4]["increment"], 768000 - 3 * 45255);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_returns_every_type_holding_an_explicit_value() -> anyhow::Result<()> {
            // Attribute 1971 jumpFatigueMultiplier: exactly five fixture Types store
            // one (Badger, Hoarder, Redeemer, Sin, Python), at 0.1 or 0.25.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 1971}}),
                )
                .await?;
            let rows = r["types"].as_array().unwrap();
            let ids: Vec<u64> = rows
                .iter()
                .map(|t| t["type_id"].as_u64().unwrap())
                .collect();
            assert_eq!(ids, vec![648, 651, 22428, 22430, 85236]);
            assert_eq!(rows[0]["value"], 0.1);
            assert_eq!(rows[4]["value"], 0.25);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_omits_types_with_no_explicit_value_for_the_attribute()
        -> anyhow::Result<()> {
            // The Ferox has a typeDogma row and stores attribute 9, so its absence
            // from the 1971 answer is sparseness of ExplicitValues, not of dogma.
            let seam = Seam::boot().await?;
            let carries_hp = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 9}}),
                )
                .await?;
            assert!(
                ids_of(&carries_hp).contains(&16227),
                "Ferox stores attribute 9"
            );

            let carries_fatigue = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 1971}}),
                )
                .await?;
            assert!(
                !ids_of(&carries_fatigue).contains(&16227),
                "Ferox stores no ExplicitValue for 1971 and must not be listed"
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_comparison_operators_filter_on_the_stored_value() -> anyhow::Result<()>
        {
            // Attribute 1971 splits 0.1 (Badger, Hoarder) from 0.25 (Redeemer, Sin,
            // Python), so each operator has a distinct expected answer.
            let seam = Seam::boot().await?;
            let cases = [
                (
                    serde_json::json!({"id": 1971, "op": "gt", "value": 0.2}),
                    vec![22428, 22430, 85236],
                ),
                (
                    serde_json::json!({"id": 1971, "op": "gte", "value": 0.25}),
                    vec![22428, 22430, 85236],
                ),
                (
                    serde_json::json!({"id": 1971, "op": "lt", "value": 0.25}),
                    vec![648, 651],
                ),
                (
                    serde_json::json!({"id": 1971, "op": "lte", "value": 0.1}),
                    vec![648, 651],
                ),
                (
                    serde_json::json!({"id": 1971, "op": "eq", "value": 0.1}),
                    vec![648, 651],
                ),
                (
                    serde_json::json!({"id": 1971, "op": "ne", "value": 0.1}),
                    vec![22428, 22430, 85236],
                ),
            ];
            for (predicate, expected) in cases {
                let r = seam
                    .call(
                        "sde_find_types",
                        serde_json::json!({"attribute": predicate.clone()}),
                    )
                    .await?;
                assert_eq!(ids_of(&r), expected, "predicate {predicate}");
            }
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_eq_matches_a_value_the_sde_stores_at_full_precision()
        -> anyhow::Result<()> {
            // The Hobgoblin II's damageMultiplier is 1.92. Widening the stored value
            // to f64 would make it 1.9199999570846558 and this eq would find nothing.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 64, "op": "eq", "value": 1.92}}),
                )
                .await?;
            assert_eq!(ids_of(&r), vec![2456]);
            assert_eq!(r["types"][0]["value"], 1.92);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_rejects_a_comparison_operator_with_no_value() -> anyhow::Result<()> {
            // Silently degrading to `exists` would answer a far broader question
            // than the caller asked.
            let seam = Seam::boot().await?;
            let r = seam
                .try_call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 1971, "op": "gt"}}),
                )
                .await;
            assert!(r.is_err(), "gt with no value must be rejected");
            assert!(format!("{}", r.unwrap_err()).contains("value"));
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_not_default_drops_values_that_restate_the_default() -> anyhow::Result<()>
        {
            // damageMultiplier defaults to 1.0 and five fixture Types store it: four
            // mining drones at exactly 1.0, the Hobgoblin II at 1.92. `exists` keeps
            // all five, `not_default` keeps only the one that says something.
            let seam = Seam::boot().await?;
            let exists = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 64}}),
                )
                .await?;
            assert_eq!(ids_of(&exists), vec![1202, 2456, 3218, 10248, 10252]);

            let not_default = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 64, "op": "not_default"}}),
                )
                .await?;
            assert_eq!(ids_of(&not_default), vec![2456]);
            assert_eq!(not_default["attribute_default"], 1.0);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_states_explicit_value_semantics_and_echoes_the_default()
        -> anyhow::Result<()> {
            // Attribute 1971 defaults to 1.0 and no fixture Type sits there, so a
            // caller reading the five rows as "everything else has no fatigue
            // multiplier" would be wrong — the envelope has to say which it is.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 1971}}),
                )
                .await?;
            let semantics = r["attribute_semantics"].as_str().expect("semantics stated");
            assert!(semantics.contains("ExplicitValue"), "{semantics}");
            assert!(semantics.contains("attribute_default"), "{semantics}");
            assert_eq!(r["attribute_default"], 1.0);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_rows_carry_id_name_and_group() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 1971, "op": "gt", "value": 0.2}}),
                )
                .await?;
            let redeemer = &r["types"][0];
            assert_eq!(redeemer["type_id"], 22428);
            assert_eq!(redeemer["name"], "Redeemer");
            assert_eq!(redeemer["group_id"], 898);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_names_stay_single_language_with_no_server_language()
        -> anyhow::Result<()> {
            // With `--language` unset every other tool returns the full localized
            // map. A selector row must not: 66 Types × eight languages is the 454 KB
            // answer this tool exists to avoid. `sde_get_type` is asserted alongside
            // to show the divergence is this tool's, not the fixture's.
            let seam = Seam::boot_with_language(None).await?;
            let full = seam
                .call("sde_get_type", serde_json::json!({"type_id": 22428}))
                .await?;
            assert!(
                full["name"].is_object(),
                "all-languages mode still returns a map elsewhere"
            );

            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 1971, "op": "gt", "value": 0.2}}),
                )
                .await?;
            assert_eq!(r["types"][0]["name"], "Redeemer");
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_reports_matched_returned_and_truncated() -> anyhow::Result<()> {
            // Eleven fixture Types store attribute 9. Under a limit of 3 the caller
            // must still be told there are eleven, or a capped page reads as the
            // whole answer.
            let seam = Seam::boot().await?;
            let capped = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 9}, "limit": 3}),
                )
                .await?;
            assert_eq!(capped["types"].as_array().unwrap().len(), 3);
            assert_eq!(capped["returned"], 3);
            assert_eq!(capped["total_matched"], 11);
            assert_eq!(capped["truncated"], true);

            let complete = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 9}}),
                )
                .await?;
            assert_eq!(complete["returned"], 11);
            assert_eq!(complete["total_matched"], 11);
            assert_eq!(complete["truncated"], false);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_orders_results_identically_across_scans() -> anyhow::Result<()> {
            // Two independent scans, because the ordering hazard is HashMap
            // iteration order, which is stable within a process and varies between
            // them. Truncation makes an unstable order silently drop different rows.
            let first = Seam::boot().await?;
            let a = first
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 9}, "limit": 4}),
                )
                .await?;
            first.shutdown().await?;

            let second = Seam::boot().await?;
            let b = second
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 9}, "limit": 4}),
                )
                .await?;
            let c = second
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 9}, "limit": 4}),
                )
                .await?;
            assert_eq!(ids_of(&a), ids_of(&b));
            assert_eq!(ids_of(&b), ids_of(&c));
            assert_eq!(ids_of(&a), vec![648, 651, 1202, 2456]);
            second.shutdown().await
        }

        #[tokio::test]
        async fn find_types_with_no_predicate_errors_instead_of_dumping() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam.try_call("sde_find_types", serde_json::json!({})).await;
            assert!(r.is_err(), "an unfiltered call must not return every Type");
            let message = format!("{}", r.unwrap_err());
            assert!(
                message.contains("attribute"),
                "names the predicates: {message}"
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_errors_on_an_undeclared_attribute() -> anyhow::Result<()> {
            // A confident empty answer for a typo'd attribute ID is the exact
            // failure mode this tool was built to end.
            let seam = Seam::boot().await?;
            let r = seam
                .try_call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 999999}}),
                )
                .await;
            assert!(r.is_err());
            assert!(format!("{}", r.unwrap_err()).contains("999999"));
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_rejects_an_unknown_operator() -> anyhow::Result<()> {
            // Falling back to `exists` on a misspelled op would answer a different
            // question under the caller's own words.
            let seam = Seam::boot().await?;
            let r = seam
                .try_call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 64, "op": "neq", "value": 1.0}}),
                )
                .await;
            assert!(r.is_err());
            assert!(format!("{}", r.unwrap_err()).contains("not_default"));
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_lists_the_types_in_a_group() -> anyhow::Result<()> {
            // "All Black Ops hulls" — group 898 holds three fixture Types. A Group
            // record names none of its Types, so before this predicate the question
            // had no answer at all.
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_find_types", serde_json::json!({"group_ids": [898]}))
                .await?;
            assert_eq!(ids_of(&r), vec![22428, 22430, 85236]);
            assert_eq!(r["total_matched"], 3);
            assert_eq!(r["truncated"], false);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_unions_several_groups_in_type_id_order() -> anyhow::Result<()> {
            // Groups 898 and 28 are named highest-first and hold interleaving ID
            // ranges, so a union that just concatenated the two scan-time runs
            // would come back out of order — and truncation would then drop
            // whichever rows the set happened to iterate last.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"group_ids": [898, 28]}),
                )
                .await?;
            assert_eq!(ids_of(&r), vec![648, 651, 22428, 22430, 85236]);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_lists_a_category_through_its_groups() -> anyhow::Result<()> {
            // "All ships": category 6 owns no Types directly — it reaches them
            // through groups 27, 28, 419, 463 and 898.
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_find_types", serde_json::json!({"category_ids": [6]}))
                .await?;
            assert_eq!(
                ids_of(&r),
                vec![638, 648, 651, 16227, 17476, 22428, 22430, 85236]
            );
            assert_eq!(r["total_matched"], 8);

            // Several Categories at once, spanning three Groups across two of them.
            let several = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"category_ids": [18, 25]}),
                )
                .await?;
            assert_eq!(ids_of(&several), vec![1202, 1230, 2456, 3218, 10248, 10252]);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_ands_the_group_and_category_filters() -> anyhow::Result<()> {
            // Group 27 Battleship is a Ship, group 101 Mining Drone is not. Naming
            // both Groups and category 6 must keep only the Battleship — the
            // predicates AND, they do not union into "ships or mining drones".
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"group_ids": [27, 101], "category_ids": [6]}),
                )
                .await?;
            assert_eq!(ids_of(&r), vec![638]);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_errors_on_an_undeclared_group_or_category() -> anyhow::Result<()> {
            // Same contract as an undeclared attribute: a typo'd taxonomy ID must
            // not come back as a confident empty answer.
            let seam = Seam::boot().await?;
            let group = seam
                .try_call("sde_find_types", serde_json::json!({"group_ids": [999999]}))
                .await;
            assert!(group.is_err());
            assert!(format!("{}", group.unwrap_err()).contains("999999"));

            let category = seam
                .try_call(
                    "sde_find_types",
                    serde_json::json!({"category_ids": [999999]}),
                )
                .await;
            assert!(category.is_err());
            assert!(format!("{}", category.unwrap_err()).contains("999999"));
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_published_only_applies_before_the_limit() -> anyhow::Result<()> {
            // Groups 101 and 898 hold 1202, 3218, 10248, 10252, 22428, 22430, 85236
            // in that order, and 10248/10252 are unpublished. Filtering after the
            // limit would take the first four, discard two of them and answer a
            // request for four published Types with two.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({
                        "group_ids": [101, 898], "published_only": true, "limit": 4
                    }),
                )
                .await?;
            assert_eq!(ids_of(&r), vec![1202, 3218, 22428, 22430]);
            assert_eq!(r["returned"], 4);
            assert_eq!(r["total_matched"], 5);
            assert_eq!(r["truncated"], true);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_composes_taxonomy_with_the_attribute_predicate() -> anyhow::Result<()> {
            // "Ships with a non-default jump fatigue multiplier" — the query the
            // ADR was written for — in one call rather than a client-side join.
            // Attribute 1971 is also carried by nothing outside category 6 here, so
            // group 898 is what proves the taxonomy half is doing work.
            let seam = Seam::boot().await?;
            let ships = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({
                        "attribute": {"id": 1971, "op": "not_default"}, "category_ids": [6]
                    }),
                )
                .await?;
            assert_eq!(ids_of(&ships), vec![648, 651, 22428, 22430, 85236]);

            let black_ops = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({
                        "attribute": {"id": 1971, "op": "not_default"}, "group_ids": [898]
                    }),
                )
                .await?;
            assert_eq!(ids_of(&black_ops), vec![22428, 22430, 85236]);
            // The ExplicitValue still rides along, and the envelope still states
            // which semantics produced the rows.
            assert_eq!(black_ops["types"][0]["value"], 0.25);
            assert_eq!(black_ops["attribute_default"], 1.0);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_published_only_narrows_an_attribute_predicate() -> anyhow::Result<()> {
            // Attribute 64 is stored by two unpublished mining drones. `published_only`
            // has to reach the attribute candidate set too, not just the taxonomy one.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 64}, "published_only": true}),
                )
                .await?;
            assert_eq!(ids_of(&r), vec![1202, 2456, 3218]);
            assert_eq!(r["total_matched"], 3);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_published_only_is_not_a_predicate_on_its_own() -> anyhow::Result<()> {
            // It narrows a candidate set; it cannot produce one. Accepting it alone
            // would dump every published Type under the guise of a filtered query.
            let seam = Seam::boot().await?;
            let r = seam
                .try_call(
                    "sde_find_types",
                    serde_json::json!({"published_only": true}),
                )
                .await;
            assert!(r.is_err(), "published_only alone must not dump every Type");
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_filters_by_meta_group_and_reports_the_absences() -> anyhow::Result<()> {
            // The five Types carrying attribute 1971 are Badger (MetaGroup 1),
            // Hoarder (none at all), Redeemer and Sin (2) and Python (4). Scoping to
            // Tech II keeps the two Black Ops; the Hoarder is not one of them, but
            // neither is it a non-match — it has no MetaGroup to judge, and saying so
            // is the difference between "these are the T2 ones" and "one candidate
            // could not be classified".
            let seam = Seam::boot().await?;
            let tech_two = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 1971}, "meta_group_ids": [2]}),
                )
                .await?;
            assert_eq!(ids_of(&tech_two), vec![22428, 22430]);
            assert_eq!(tech_two["total_matched"], 2);
            assert_eq!(tech_two["excluded_no_meta_group"], 1);

            // The same call for Tech I: the Hoarder is still excluded and still
            // counted, because absence of a MetaGroup must never read as Tech I.
            let tech_one = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 1971}, "meta_group_ids": [1]}),
                )
                .await?;
            assert_eq!(ids_of(&tech_one), vec![648]);
            assert_eq!(tech_one["excluded_no_meta_group"], 1);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_composes_the_meta_group_filter_with_taxonomy() -> anyhow::Result<()> {
            // Group 101 Mining Drone holds the Civilian (MetaGroup 1), the Harvester
            // (4) and the two unnamed-tier drones (none). Scoping to Tech I keeps the
            // Civilian; the Harvester is a definite non-match and is *not* counted,
            // while the two with no MetaGroup are — a wrong tier and no tier are
            // different answers.
            let seam = Seam::boot().await?;
            let tech_one_drones = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"group_ids": [101], "meta_group_ids": [1]}),
                )
                .await?;
            assert_eq!(ids_of(&tech_one_drones), vec![1202]);
            assert_eq!(tech_one_drones["excluded_no_meta_group"], 2);

            // Category 6 Ship reaches its Types through five Groups; only the two
            // Black Ops are Tech II, and only the Hoarder has no MetaGroup.
            let tech_two_ships = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"category_ids": [6], "meta_group_ids": [2]}),
                )
                .await?;
            assert_eq!(ids_of(&tech_two_ships), vec![22428, 22430]);
            assert_eq!(tech_two_ships["excluded_no_meta_group"], 1);

            // Several MetaGroups at once, ORed among themselves the way group_ids is.
            let tiered_ships = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"category_ids": [6], "meta_group_ids": [2, 4]}),
                )
                .await?;
            assert_eq!(ids_of(&tiered_ships), vec![22428, 22430, 85236]);
            assert_eq!(tiered_ships["excluded_no_meta_group"], 1);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_counts_meta_group_absences_across_the_full_candidate_set()
        -> anyhow::Result<()> {
            // Group 18 Mineral's eight Types have no MetaGroup and group 898's three
            // do. Asking for the Tech II ones under a limit of 1 returns a single row
            // — a count scoped to the page could report at most that one, and a count
            // scoped to the match set at most two. Eight can only come from every
            // candidate the filter actually judged.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({
                        "group_ids": [18, 898], "meta_group_ids": [2], "limit": 1
                    }),
                )
                .await?;
            assert_eq!(ids_of(&r), vec![22428]);
            assert_eq!(r["returned"], 1);
            assert_eq!(r["total_matched"], 2);
            assert_eq!(r["truncated"], true);
            assert_eq!(r["excluded_no_meta_group"], 8);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_counts_only_absences_the_other_predicates_left_standing()
        -> anyhow::Result<()> {
            // Both MetaGroup-less Types in group 101 are unpublished. With
            // published_only they were already out of the running, so counting them
            // as "excluded for having no MetaGroup" would overstate how much of the
            // caller's own question went unanswered.
            let seam = Seam::boot().await?;
            let everything = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"group_ids": [101], "meta_group_ids": [1, 4]}),
                )
                .await?;
            assert_eq!(ids_of(&everything), vec![1202, 3218]);
            assert_eq!(everything["excluded_no_meta_group"], 2);

            let published = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({
                        "group_ids": [101], "meta_group_ids": [1, 4], "published_only": true
                    }),
                )
                .await?;
            assert_eq!(ids_of(&published), vec![1202, 3218]);
            assert_eq!(published["excluded_no_meta_group"], 0);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_reports_no_meta_group_count_when_the_filter_is_inactive()
        -> anyhow::Result<()> {
            // Absent rather than zero: the same call returns Types with and without a
            // MetaGroup, so a `0` here would claim nothing was dropped for a filter
            // that never ran.
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_find_types", serde_json::json!({"group_ids": [101]}))
                .await?;
            assert_eq!(ids_of(&r), vec![1202, 3218, 10248, 10252]);
            assert!(
                r.get("excluded_no_meta_group").is_none(),
                "no MetaGroup filter ran: {r}"
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_meta_group_is_not_a_predicate_on_its_own() -> anyhow::Result<()> {
            // Like published_only, it narrows a candidate set rather than producing
            // one: the store maps Type → MetaGroup and not back, because "every Tech
            // II item in EVE" is not a question this tool answers.
            let seam = Seam::boot().await?;
            let r = seam
                .try_call("sde_find_types", serde_json::json!({"meta_group_ids": [2]}))
                .await;
            assert!(r.is_err(), "meta_group_ids alone must not dump every Type");
            let message = format!("{}", r.unwrap_err());
            assert!(
                message.contains("meta_group_ids"),
                "says why it was not enough: {message}"
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_rolls_up_the_groups_it_matched() -> anyhow::Result<()> {
            // Group 18 Mineral holds eight Types and nothing else does, so the
            // rollup is one named row that accounts for every match.
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_find_types", serde_json::json!({"group_ids": [18]}))
                .await?;
            assert_eq!(
                r["groups"],
                serde_json::json!([{"group_id": 18, "name": "Mineral", "count": 8}])
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_rollup_counts_the_full_match_set_not_the_page() -> anyhow::Result<()> {
            // Two of category 6's eight Ships are returned, and they sit in groups
            // 27 and 28. A rollup derived from the returned rows would report a
            // two-Group Category; the real answer is five Groups totalling eight,
            // and reporting the page as the taxonomy is the specific failure this
            // rollup replaces `list_groups_in_category` to avoid.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"category_ids": [6], "limit": 2}),
                )
                .await?;
            assert_eq!(ids_of(&r), vec![638, 648]);
            assert_eq!(r["truncated"], true);
            assert_eq!(
                r["groups"],
                serde_json::json!([
                    {"group_id": 898, "name": "Black Ops", "count": 3},
                    {"group_id": 28, "name": "Hauler", "count": 2},
                    {"group_id": 27, "name": "Battleship", "count": 1},
                    {"group_id": 419, "name": "Combat Battlecruiser", "count": 1},
                    {"group_id": 463, "name": "Mining Barge", "count": 1},
                ])
            );
            let summed: u64 = r["groups"]
                .as_array()
                .unwrap()
                .iter()
                .map(|g| g["count"].as_u64().unwrap())
                .sum();
            assert_eq!(summed, r["total_matched"].as_u64().unwrap());
            assert_ne!(summed, r["returned"].as_u64().unwrap());
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_rolls_up_an_attribute_only_result() -> anyhow::Result<()> {
            // "Which Groups carry an ExplicitValue for 1971" answered as a
            // by-product, with no taxonomy predicate in the call at all.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 1971}}),
                )
                .await?;
            assert_eq!(
                r["groups"],
                serde_json::json!([
                    {"group_id": 898, "name": "Black Ops", "count": 3},
                    {"group_id": 28, "name": "Hauler", "count": 2},
                ])
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_query_narrows_by_name_substring() -> anyhow::Result<()> {
            // Category 18 Drone holds five Types; only four are named for mining, so
            // the Hobgoblin II is what the substring has to remove.
            let seam = Seam::boot().await?;
            let all_drones = seam
                .call("sde_find_types", serde_json::json!({"category_ids": [18]}))
                .await?;
            assert_eq!(ids_of(&all_drones), vec![1202, 2456, 3218, 10248, 10252]);

            let mining = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"category_ids": [18], "query": "mining"}),
                )
                .await?;
            assert_eq!(ids_of(&mining), vec![1202, 3218, 10248, 10252]);
            assert_eq!(mining["total_matched"], 4);

            // The names are "Mining Drone", not "mining drone".
            let shouted = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"category_ids": [18], "query": "MINING"}),
                )
                .await?;
            assert_eq!(ids_of(&shouted), ids_of(&mining));
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_query_composes_with_every_other_predicate() -> anyhow::Result<()> {
            // "Which published drones named for mining record a damageMultiplier" —
            // an attribute predicate, a Category, a name substring and published_only
            // in one call. The two unpublished mining drones are the ones only
            // published_only removes; the Hobgoblin II is the one only `query` does.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({
                        "attribute": {"id": 64},
                        "category_ids": [18],
                        "query": "mining",
                        "published_only": true
                    }),
                )
                .await?;
            assert_eq!(ids_of(&r), vec![1202, 3218]);
            assert_eq!(r["total_matched"], 2);
            // The rollup still counts the full match set, and `query` is part of it.
            assert_eq!(
                r["groups"],
                serde_json::json!([
                    {"group_id": 101, "name": "Mining Drone", "count": 2},
                ])
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_query_reaches_every_type_sharing_a_name() -> anyhow::Result<()> {
            // The `query` predicate resolves a substring through the same
            // `name_index`, so it lost the same colliding Types sde_search_types
            // did. Both "Badger Wiyrkomi SKIN"s are in the answer.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"category_ids": [91], "query": "wiyrkomi"}),
                )
                .await?;
            assert_eq!(ids_of(&r), vec![36333, 60106]);
            assert_eq!(r["total_matched"], 2);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_query_is_not_a_predicate_on_its_own() -> anyhow::Result<()> {
            // It narrows a candidate set; sde_search_types is the bare name search.
            let seam = Seam::boot().await?;
            let r = seam
                .try_call("sde_find_types", serde_json::json!({"query": "mining"}))
                .await;
            assert!(r.is_err(), "query alone must not produce a candidate set");
            let message = format!("{}", r.unwrap_err());
            assert!(
                message.contains("query"),
                "names query as narrowing: {message}"
            );
            assert!(
                message.contains("group_ids, category_ids, type_ids"),
                "and lists type_ids among the predicates that do produce one: {message}"
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_type_ids_restricts_evaluation_to_the_given_set() -> anyhow::Result<()> {
            // "Of these Types I already hold, which record jump fatigue?" Five
            // fixture Types do; naming three of them plus one that records none plus
            // one the SDE never declared must answer with the two that qualify, and
            // must not fail on the undeclared ID.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({
                        "attribute": {"id": 1971},
                        "type_ids": [651, 16227, 22428, 999999]
                    }),
                )
                .await?;
            assert_eq!(ids_of(&r), vec![651, 22428]);
            assert_eq!(r["total_matched"], 2);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_type_ids_composes_with_taxonomy() -> anyhow::Result<()> {
            // The set spans a ship and a drone; scoping to Category 6 keeps the ship.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"category_ids": [6], "type_ids": [648, 1202]}),
                )
                .await?;
            assert_eq!(ids_of(&r), vec![648]);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_type_ids_produces_a_candidate_set_on_its_own() -> anyhow::Result<()> {
            // The one narrowing predicate that can stand alone: producing from it is
            // bounded by what the caller typed, not by a scan. Rows come back in
            // type ID order like every other answer rather than in the order the IDs
            // arrived, and a repeated ID is one row.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"type_ids": [651, 648, 651]}),
                )
                .await?;
            assert_eq!(ids_of(&r), vec![648, 651]);
            assert_eq!(r["total_matched"], 2);
            assert_eq!(r["types"][0]["name"], "Badger");
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_type_ids_alone_narrows_by_published_only() -> anyhow::Result<()> {
            // "Which of the ones I hold are published" — cheap, bounded, and an
            // error until type_ids could produce a candidate set.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({
                        "type_ids": [1202, 3218, 10248, 10252], "published_only": true
                    }),
                )
                .await?;
            assert_eq!(ids_of(&r), vec![1202, 3218]);
            assert_eq!(r["total_matched"], 2);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_type_ids_drops_an_id_the_sde_does_not_declare() -> anyhow::Result<()> {
            // Producing from the caller's list must not invent a row: an undeclared
            // ID has no name and no Group, and emitting it as nulls would read as a
            // Type that exists and could not be described.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"type_ids": [648, 999999]}),
                )
                .await?;
            assert_eq!(ids_of(&r), vec![648]);
            assert_eq!(r["total_matched"], 1);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_projects_the_attributes_it_was_asked_for() -> anyhow::Result<()> {
            // The motivating call, in miniature: select a set, read two fields off
            // every row, no follow-up batch dogma call.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({
                        "group_ids": [101], "project_attributes": [64, 77]
                    }),
                )
                .await?;
            assert_eq!(ids_of(&r), vec![1202, 3218, 10248, 10252]);
            assert_eq!(
                r["types"][0]["attributes"],
                serde_json::json!({"64": 1.0, "77": 13.0})
            );
            assert_eq!(
                r["types"][1]["attributes"],
                serde_json::json!({"64": 1.0, "77": 42.0})
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_projection_reports_an_absent_explicit_value_as_absent()
        -> anyhow::Result<()> {
            // The Hobgoblin II records damageMultiplier and no miningAmount. Its map
            // must carry the one and simply not mention the other — reporting 77 as
            // 0, or as its DefaultValue, is the confusion this whole tool exists to
            // end. A Type recording none of them gets an empty map, not a missing
            // key: the projection ran, and that is its answer.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({
                        "group_ids": [100, 101], "project_attributes": [64, 77]
                    }),
                )
                .await?;
            let hobgoblin = r["types"]
                .as_array()
                .unwrap()
                .iter()
                .find(|t| t["type_id"] == 2456)
                .expect("Hobgoblin II");
            assert_eq!(hobgoblin["attributes"], serde_json::json!({"64": 1.92}));
            assert!(
                hobgoblin["attributes"].get("77").is_none(),
                "no ExplicitValue for 77 means no key: {hobgoblin}"
            );

            let nothing_recorded = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({
                        "group_ids": [101], "project_attributes": [1971]
                    }),
                )
                .await?;
            assert_eq!(
                nothing_recorded["types"][0]["attributes"],
                serde_json::json!({})
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_rejects_a_projected_attribute_the_sde_does_not_declare()
        -> anyhow::Result<()> {
            // A projection is not a predicate, but a typo'd ID in one still produces
            // a confidently empty answer, so it is rejected the way an undeclared
            // Group is — and by the same helper sde_get_types_dogma uses, so the two
            // tools cannot drift.
            let seam = Seam::boot().await?;
            let r = seam
                .try_call(
                    "sde_find_types",
                    serde_json::json!({"group_ids": [101], "project_attributes": [64, 999999]}),
                )
                .await;
            assert!(r.is_err(), "an undeclared attribute must be rejected");
            assert!(format!("{}", r.unwrap_err()).contains("999999"));
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_reads_an_empty_projection_list_as_no_projection() -> anyhow::Result<()>
        {
            // Naming nothing is not the same as projecting nothing: the `attributes`
            // key is omitted rather than emitted empty, so an empty map keeps its
            // one meaning — this Type records none of the attributes you named.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"group_ids": [101], "project_attributes": []}),
                )
                .await?;
            assert_eq!(ids_of(&r), vec![1202, 3218, 10248, 10252]);
            assert!(
                r["types"][0].get("attributes").is_none(),
                "an empty list names no projection: {r}"
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_omits_the_attributes_map_when_no_projection_ran() -> anyhow::Result<()>
        {
            // Absent rather than empty, so an empty map keeps meaning "this Type
            // records none of the attributes you named".
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_find_types", serde_json::json!({"group_ids": [101]}))
                .await?;
            assert!(
                r["types"][0].get("attributes").is_none(),
                "no projection was asked for: {r}"
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_projection_changes_no_row_and_no_count() -> anyhow::Result<()> {
            // A projection widens rows; it is not a predicate. Same rows, same order,
            // same total_matched, same rollup — including under truncation, where the
            // projected page must still describe the full match set.
            let seam = Seam::boot().await?;
            let plain = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"category_ids": [18], "limit": 2}),
                )
                .await?;
            let projected = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({
                        "category_ids": [18], "limit": 2, "project_attributes": [64, 77]
                    }),
                )
                .await?;
            assert_eq!(ids_of(&projected), ids_of(&plain));
            assert_eq!(projected["total_matched"], plain["total_matched"]);
            assert_eq!(projected["returned"], plain["returned"]);
            assert_eq!(projected["truncated"], plain["truncated"]);
            assert_eq!(projected["groups"], plain["groups"]);

            // Only the returned rows are projected — the two the page cut carry no
            // map because they were never built.
            assert_eq!(projected["total_matched"], 5);
            assert_eq!(projected["types"].as_array().unwrap().len(), 2);
            assert_eq!(
                projected["types"][0]["attributes"],
                serde_json::json!({"64": 1.0, "77": 13.0})
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_modifiers_points_at_the_selector_when_nothing_modifies() -> anyhow::Result<()>
        {
            // The exact wrong turn that motivated the epic, reproduced: attribute 1971
            // is modified by nothing and carried by Types, so a bare empty answer is
            // what an agent previously read as "the SDE has no data".
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_get_modifiers",
                    serde_json::json!({"attribute_id": 1971}),
                )
                .await?;
            assert_eq!(
                r["modified_by"].as_array().unwrap().len(),
                0,
                "fixture pins 1971 as modified by nothing"
            );
            // The count is the whole point: it tells the caller the other tool has an
            // answer before they spend a call finding out.
            assert_eq!(r["explicit_type_count"], 5);
            let g = r["guidance"].as_str().expect("guidance on an empty answer");
            assert!(
                g.contains("sde_find_types"),
                "names the tool that answers: {g}"
            );
            assert!(g.contains('5'), "quotes the count of carriers: {g}");
            seam.shutdown().await
        }

        #[tokio::test]
        async fn get_modifiers_stays_quiet_when_it_has_an_answer() -> anyhow::Result<()> {
            // Self-description keys appear exactly when the thing they describe ran.
            // Guidance on a non-empty answer would be noise a caller learns to skip.
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_get_modifiers", serde_json::json!({"attribute_id": 77}))
                .await?;
            assert!(!r["modified_by"].as_array().unwrap().is_empty());
            assert!(r.get("guidance").is_none(), "no guidance when not confused");
            assert!(r.get("explicit_type_count").is_none());
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_points_at_modifiers_when_an_empty_answer_may_be_the_wrong_question()
        -> anyhow::Result<()> {
            // The reciprocal direction: a predicate that matched nothing, on an
            // attribute something does modify — so MODIFIES may be what was wanted.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 77, "op": "eq", "value": 999999.0}}),
                )
                .await?;
            assert_eq!(r["total_matched"], 0);
            let g = r["guidance"].as_str().expect("guidance on an empty match");
            assert!(g.contains("sde_get_modifiers"), "names the other tool: {g}");
            assert!(
                g.contains("DefaultValue"),
                "restates ExplicitValue-only: {g}"
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_blames_the_predicates_not_the_attribute_when_carriers_exist()
        -> anyhow::Result<()> {
            // Five fixture Types record attribute 1971 and none is above 999. The
            // count comes off the inverted index, before the operator ran, so the
            // response cannot claim the attribute is unrecorded when it is recorded
            // and merely filtered out — the mis-signal #37 was opened for.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 1971, "op": "gt", "value": 999.0}}),
                )
                .await?;
            assert_eq!(r["total_matched"], 0);
            let g = r["guidance"].as_str().expect("guidance on an empty match");
            assert!(
                !g.contains("No Type records"),
                "must not deny the five stored rows: {g}"
            );
            assert!(g.contains("5 Types record"), "quotes the true count: {g}");

            // Same correction when it is a narrowing predicate, not the operator,
            // that empties the set: group 18 Mineral carries no 1971 at all.
            let narrowed = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 1971}, "group_ids": [18]}),
                )
                .await?;
            assert_eq!(narrowed["total_matched"], 0);
            let g = narrowed["guidance"]
                .as_str()
                .expect("guidance on an empty match");
            assert!(g.contains("5 Types record"), "quotes the true count: {g}");
            assert!(g.contains("group_ids"), "names the predicate to relax: {g}");
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_says_so_when_nothing_modifies_it_either() -> anyhow::Result<()> {
            // Attribute 30 is declared, carried by no Type and modified by nothing.
            // Pointing at sde_get_modifiers here would send the caller to a second
            // empty answer, so the guidance sends them to check the attribute instead.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"attribute": {"id": 30}}),
                )
                .await?;
            assert_eq!(r["total_matched"], 0);
            let g = r["guidance"].as_str().expect("guidance on an empty match");
            assert!(
                !g.contains("sde_get_modifiers"),
                "must not route to a tool that is also empty: {g}"
            );
            assert!(
                g.contains("sde_search_dogma"),
                "routes to verification: {g}"
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_tells_a_truncated_caller_how_to_narrow() -> anyhow::Result<()> {
            // Epic story 35: the call succeeded, matched more than the page, and the
            // `groups` rollup is already the right narrowing axis — but nothing said so.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_find_types",
                    serde_json::json!({"category_ids": [18], "limit": 2}),
                )
                .await?;
            assert_eq!(r["truncated"], true);
            let g = r["guidance"]
                .as_str()
                .expect("guidance on a truncated page");
            assert!(g.contains("groups"), "names the rollup as the axis: {g}");
            assert!(
                g.contains("group_ids"),
                "names a predicate that exists: {g}"
            );
            // Offset pagination is Out of Scope in #37; guidance must not invent it.
            assert!(
                !g.contains("offset parameter to page"),
                "must not promise paging: {g}"
            );
            // The count quoted is always the full match set, never the page.
            assert!(g.contains("5 Types matched"), "quotes total_matched: {g}");
            seam.shutdown().await
        }

        #[tokio::test]
        async fn find_types_stays_quiet_on_a_complete_answer() -> anyhow::Result<()> {
            // Neither empty nor truncated: nothing to warn about, so no key.
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_find_types", serde_json::json!({"category_ids": [18]}))
                .await?;
            assert_eq!(r["truncated"], false);
            assert!(r["total_matched"].as_u64().unwrap() > 0);
            assert!(r.get("guidance").is_none(), "no guidance when not confused");
            seam.shutdown().await
        }

        /// The `attribute_id`s of a `sde_search_dogma` answer, in the order returned.
        fn attribute_ids(response: &serde_json::Value) -> Vec<u64> {
            response["attributes"]
                .as_array()
                .expect("attributes array")
                .iter()
                .map(|a| a["attribute_id"].as_u64().expect("attribute_id"))
                .collect()
        }

        /// The one hit for `attribute_id`, or a panic naming what came back instead.
        fn attribute_hit(response: &serde_json::Value, attribute_id: u64) -> &serde_json::Value {
            response["attributes"]
                .as_array()
                .expect("attributes array")
                .iter()
                .find(|a| a["attribute_id"] == attribute_id)
                .unwrap_or_else(|| panic!("attribute {attribute_id} missing from {response}"))
        }

        #[tokio::test]
        async fn search_dogma_finds_an_attribute_by_a_phrase_only_its_display_name_carries()
        -> anyhow::Result<()> {
            // Attribute 9 is named `hp` and described as "The maximum hitpoints of
            // an object." — "structure hitpoints" is in neither. It reaches the
            // caller only through the display name, which is the case that makes
            // searching one field a broken search rather than a narrow one.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_search_dogma",
                    serde_json::json!({"query": "structure hitpoints"}),
                )
                .await?;
            assert_eq!(attribute_ids(&r), vec![9]);
            let hit = attribute_hit(&r, 9);
            assert_eq!(hit["name"], "hp");
            assert_eq!(hit["display_name"], "Structure Hitpoints");
            assert_eq!(hit["matched_fields"], serde_json::json!(["display_name"]));
            seam.shutdown().await
        }

        #[tokio::test]
        async fn search_dogma_reports_every_field_a_hit_matched_on() -> anyhow::Result<()> {
            // "jump fatigue" is in 1971's display name AND its description, and in
            // neither case in the camelCase `jumpFatigueMultiplier` — the phrasing
            // the motivating session reached for first.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_search_dogma",
                    serde_json::json!({"query": "jump fatigue"}),
                )
                .await?;
            let hit = attribute_hit(&r, 1971);
            assert_eq!(hit["name"], "jumpFatigueMultiplier");
            assert_eq!(
                hit["matched_fields"],
                serde_json::json!(["display_name", "description"]),
                "the identifier holds no space, so `name` cannot be among them"
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn search_dogma_returns_attributes_and_effects_as_distinct_lists()
        -> anyhow::Result<()> {
            // "power" names both: attributes 11 powerOutput and 30 power, effects
            // 11/12/13 loPower/hiPower/medPower. A caller who cannot tell which
            // kind of thing a term names gets both without asking twice.
            let seam = Seam::boot().await?;
            let r = seam
                .call("sde_search_dogma", serde_json::json!({"query": "power"}))
                .await?;
            assert_eq!(attribute_ids(&r), vec![11, 30]);
            let effect_ids: Vec<u64> = r["effects"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e["effect_id"].as_u64().unwrap())
                .collect();
            assert_eq!(effect_ids, vec![11, 12, 13]);
            assert_eq!(r["attributes_matched"], 2);
            assert_eq!(r["effects_matched"], 3);
            assert_eq!(r["truncated"], false);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn search_dogma_matches_case_insensitively() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let shouted = seam
                .call(
                    "sde_search_dogma",
                    serde_json::json!({"query": "JUMP FATIGUE"}),
                )
                .await?;
            let quiet = seam
                .call(
                    "sde_search_dogma",
                    serde_json::json!({"query": "jump fatigue"}),
                )
                .await?;
            assert_eq!(attribute_ids(&shouted), vec![1971]);
            assert_eq!(shouted, quiet);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn search_dogma_attribute_hits_carry_the_default_value() -> anyhow::Result<()> {
            // The DefaultValue is what a Type absent from the reverse lookup holds,
            // so it is the difference between reading 1971's absence as "no jump
            // fatigue" and as "the 1.0 everything else sits at".
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_search_dogma",
                    serde_json::json!({"query": "jumpFatigueMultiplier"}),
                )
                .await?;
            assert_eq!(attribute_hit(&r, 1971)["default_value"], 1.0);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn search_dogma_explicit_type_count_agrees_with_find_types() -> anyhow::Result<()> {
            // The count is only useful if it predicts the reverse lookup exactly:
            // it exists so a caller can skip a call, or size one before making it.
            // "multiplier" is deliberately a three-attribute answer, so this is not
            // one lucky number.
            let seam = Seam::boot().await?;
            let searched = seam
                .call(
                    "sde_search_dogma",
                    serde_json::json!({"query": "multiplier"}),
                )
                .await?;
            let hits = searched["attributes"].as_array().unwrap();
            assert_eq!(hits.len(), 3, "expected 64, 1971 and 275");
            for hit in hits {
                let attribute_id = hit["attribute_id"].as_u64().unwrap();
                let found = seam
                    .call(
                        "sde_find_types",
                        serde_json::json!({"attribute": {"id": attribute_id}, "limit": 1}),
                    )
                    .await?;
                assert_eq!(
                    hit["explicit_type_count"], found["total_matched"],
                    "attribute {attribute_id}: search and find disagree on how many \
                     Types hold an ExplicitValue"
                );
                assert_eq!(
                    hit["default_value"], found["attribute_default"],
                    "attribute {attribute_id}: search and find disagree on the DefaultValue"
                );
            }
            seam.shutdown().await
        }

        #[tokio::test]
        async fn search_dogma_omits_text_fields_a_record_does_not_have() -> anyhow::Result<()> {
            // Attributes 277/278 have no `displayName` in the real SDE and effects
            // 16/132 have neither that nor a `description`. A hit says which fields
            // it matched, so it must not claim fields the record never had.
            let seam = Seam::boot().await?;
            let attrs = seam
                .call(
                    "sde_search_dogma",
                    serde_json::json!({"query": "required skill level"}),
                )
                .await?;
            assert_eq!(attribute_ids(&attrs), vec![277, 278]);
            let hit = attribute_hit(&attrs, 277);
            assert_eq!(hit["name"], "requiredSkill1Level");
            assert!(hit.get("display_name").is_none(), "277 has no displayName");
            assert_eq!(hit["matched_fields"], serde_json::json!(["description"]));

            let effects = seam
                .call(
                    "sde_search_dogma",
                    serde_json::json!({"query": "skillEffect"}),
                )
                .await?;
            let effect = &effects["effects"].as_array().unwrap()[0];
            assert_eq!(effect["effect_id"], 132);
            assert!(effect.get("display_name").is_none());
            assert!(effect.get("description").is_none());
            assert_eq!(effect["matched_fields"], serde_json::json!(["name"]));
            seam.shutdown().await
        }

        #[tokio::test]
        async fn search_dogma_puts_name_matches_ahead_of_text_only_matches() -> anyhow::Result<()> {
            // 275 skillTimeConstant matches "multiplier" only in its display name
            // and description, so it sorts behind 1971 despite the lower ID. Under
            // a cap that ordering is what keeps the identifier hit on the page.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_search_dogma",
                    serde_json::json!({"query": "multiplier"}),
                )
                .await?;
            assert_eq!(attribute_ids(&r), vec![64, 1971, 275]);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn search_dogma_orders_hits_identically_across_scans() -> anyhow::Result<()> {
            // Two independent scans in one process still share no HashMap iteration
            // order, which is the ordering hazard: the corpus is a Vec sorted by ID
            // and the rank sort above it is stable.
            let first = Seam::boot().await?;
            let a = first
                .call("sde_search_dogma", serde_json::json!({"query": "skill"}))
                .await?;
            first.shutdown().await?;
            let second = Seam::boot().await?;
            let b = second
                .call("sde_search_dogma", serde_json::json!({"query": "skill"}))
                .await?;
            assert!(
                a["attributes"].as_array().unwrap().len() > 1,
                "an ordering assertion needs something to order"
            );
            assert_eq!(a, b);
            second.shutdown().await
        }

        #[tokio::test]
        async fn search_dogma_caps_each_list_separately_and_reports_the_totals()
        -> anyhow::Result<()> {
            // A shared cap would let two attribute hits hide all three effect hits,
            // which is exactly the guess this tool exists to remove.
            let seam = Seam::boot().await?;
            let r = seam
                .call(
                    "sde_search_dogma",
                    serde_json::json!({"query": "power", "limit": 1}),
                )
                .await?;
            assert_eq!(r["attributes_returned"], 1);
            assert_eq!(r["attributes_matched"], 2);
            assert_eq!(r["effects_returned"], 1);
            assert_eq!(r["effects_matched"], 3);
            assert_eq!(r["truncated"], true);
            seam.shutdown().await
        }

        #[tokio::test]
        async fn search_dogma_returns_english_text_in_all_languages_mode() -> anyhow::Result<()> {
            // Every other tool answers with all eight languages when `--language` is
            // unset. The corpus holds one, so a hit's display name is a plain string
            // here too — the same deliberate divergence sde_find_types makes for its
            // row names, and for the same reason.
            let seam = Seam::boot_with_language(None).await?;
            let r = seam
                .call(
                    "sde_search_dogma",
                    serde_json::json!({"query": "jump fatigue"}),
                )
                .await?;
            assert_eq!(
                attribute_hit(&r, 1971)["display_name"],
                "Jump Fatigue Multiplier"
            );
            seam.shutdown().await
        }

        #[tokio::test]
        async fn search_dogma_rejects_an_empty_query() -> anyhow::Result<()> {
            // An empty substring is inside every record, so the honest answer is a
            // correction rather than a capped page of the entire corpus.
            let seam = Seam::boot().await?;
            let r = seam
                .try_call("sde_search_dogma", serde_json::json!({"query": "   "}))
                .await;
            let message = r.unwrap_err().to_string();
            assert!(
                message.contains("non-empty query"),
                "expected a corrective error, got: {message}"
            );
            seam.shutdown().await
        }
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
