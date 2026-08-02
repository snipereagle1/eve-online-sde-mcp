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

use super::dogma::AttributeOp;
use super::guidance::{
    NARROW_ONLY_PREDICATES, SERVER_INSTRUCTIONS, STANDALONE_PREDICATES, guidance_for,
};
use super::manufacturing;
use super::query;
use super::query::pick_name;
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

    pub(crate) fn filter(&self, value: &mut serde_json::Value) {
        if let Some(ref lang) = self.language {
            query::apply_language_filter(value, lang);
        }
    }

    pub(crate) fn fetch_filtered(
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

    pub(crate) fn search_filtered(
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
    pub(crate) fn search_filtered_where(
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
    pub(crate) fn type_name(&self, id: u64) -> Option<String> {
        let v = query::fetch_by_id(&self.store.types, id).ok()?;
        pick_name(v.get("name"), self.language.as_deref())
    }

    /// Name of a dogma attribute by id, or None.
    pub(crate) fn attribute_name(&self, id: u64) -> Option<String> {
        let v = query::fetch_by_id(&self.store.dogma_attributes, id).ok()?;
        pick_name(v.get("name"), self.language.as_deref())
    }

    /// True if a type is a skill (category 16), via type→group→category. Used by the
    /// levers view to float skills above implants/boosters/ships when listing what
    /// modifies an attribute — the skill sources are what a training plan cares about.
    pub(crate) fn is_skill(&self, type_id: u64) -> bool {
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
}

// ── Tool implementations ─────────────────────────────────────────────────────

#[tool_router(router = server_router, vis = "pub(crate)")]
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

    #[tool(description = "Get a SKIN (ship SKINs) by its skin ID")]
    async fn sde_get_skin(
        &self,
        Parameters(p): Parameters<SkinIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.skins, p.skin_id, "skins")
    }
}

/// The one router the handler dispatches on, summed from each domain's. A new
/// domain module is the only thing that changes this function — a new tool inside
/// an existing one composes automatically.
impl SdeMcpServer {
    fn tool_router() -> rmcp::handler::server::router::tool::ToolRouter<Self> {
        Self::server_router()
            + Self::blueprints_router()
            + Self::dogma_router()
            + Self::map_router()
            + Self::market_router()
            + Self::politics_router()
            + Self::skills_router()
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::testkit::{default_store, make_index, make_server, write_fixture};

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
    }
}
