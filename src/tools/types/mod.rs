//! Types and their taxonomy: the Type records themselves, the Groups and
//! Categories above them, reprocessing materials, SKINs, and the Type selector.
//!
//! `sde_find_types` is the selector: predicates AND together, and only
//! `attribute` / `group_ids` / `category_ids` / `type_ids` can produce a
//! candidate set — the rest merely narrow one. Attribute reads here are
//! ExplicitValue reads, done through `dogma`'s projection helpers.

use std::collections::{BTreeMap, HashMap, HashSet};

use rmcp::{
    ErrorData,
    handler::server::wrapper::Parameters,
    schemars::{self, JsonSchema},
    tool, tool_router,
};
use serde::Deserialize;

use crate::tools::SdeMcpServer;
use crate::tools::dogma::AttributeOp;
use crate::tools::guidance::{NARROW_ONLY_PREDICATES, STANDALONE_PREDICATES, guidance_for};
use crate::tools::query;
use crate::tools::query::pick_name;

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

impl SdeMcpServer {
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
    pub(crate) fn project_explicit_values(
        &self,
        type_id: u32,
        wanted: &[u32],
    ) -> BTreeMap<u64, f32> {
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

#[tool_router(router = types_router, vis = "pub(crate)")]
impl SdeMcpServer {
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

/// The MetaGroups a call is restricted to, or `None` when it names none. Unlike
/// [`SdeMcpServer::resolve_group_filter`] this validates nothing: `metaGroups.jsonl`
/// is deliberately not scanned, so the server holds no list of declared MetaGroups
/// to check an ID against and cannot tell a typo from a MetaGroup no Type uses. An
/// ID too large to be one is dropped rather than errored for the same reason — it
/// simply matches nothing, which is what the response then reports.
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

#[cfg(test)]
mod tests;
