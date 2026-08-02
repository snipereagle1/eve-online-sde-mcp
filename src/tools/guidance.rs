//! Where the agent should go next: the handshake playbook and the narrow
//! `sde_find_types` responses that invite a wrong conclusion.
//!
//! One concern, fired at two moments — before any tool is chosen, and after a
//! result a caller is likely to misread. Both are prompt-critical text, so they
//! live together rather than beside whichever tool happens to emit them.

/// Server-level usage playbook, delivered in the initialize handshake so the model
/// reads it before choosing tools. Kept short on purpose — long instructions get skimmed.
pub(crate) const SERVER_INSTRUCTIONS: &str = "\
EVE Online Static Data Export (SDE) query server. Data is read-only game data indexed by ID.

Pick the most direct tool — most questions are ONE call, not a fan-out:
- \"What skills / what order to fly SHIP or use MODULE\" → sde_get_skill_plan with ALL target type IDs in one call. Its output already gives the topo-sorted prerequisite order, each skill's rank, per-level SP cost (sp_by_level), running cumulative SP, and which target needs it. Do NOT call sde_get_type_dogma or sde_get_skill_sp per skill to rebuild this.
- \"Which skills/ships boost ATTRIBUTE X (e.g. mining yield, attr 77)\" → sde_get_modifiers with attribute_id — one call returns every modifier, one row per owning type. Read source_type_id/source_type_name as the bonus SOURCE (e.g. Astrogeology); required_skill_id/required_skill_name is only a target-module filter, NOT the source. Use type_id for the inverse, effect_id for a single effect's modifierInfo.
- HAS vs MODIFIES — the two questions sound alike and use disjoint data. \"Which Types HAVE attribute X\" (carry a stored ExplicitValue for it) → sde_find_types with attribute {id: X}. \"Which Types MODIFY attribute X\" (boost/penalise it) → sde_get_modifiers with attribute_id X. An empty answer from either is NOT evidence the other is empty: nothing modifies attr 1971, yet 66 Types carry it. Both tools say so on an empty result and name the other — read that before concluding the SDE lacks the data.
- Finding the attribute or effect ID in the first place → sde_search_dogma by name. Needs a contiguous substring: it matches text, so a spaced phrase will not reach a camelCase-only identifier.
- Known exact names → IDs → sde_resolve_types (one bulk call). Use sde_search_types only for fuzzy/unknown-name discovery. Note sde_resolve_types answers one ID per exact name, so a name shared by several Types resolves to one of them only — the lowest published ID, or the lowest of all when none is published. Use sde_search_types when you need the whole group.
- Several types or dogma records at once → sde_get_types / sde_get_types_dogma (batched), not many single calls.
- Decoding skill prereqs from raw dogma → pass resolve_names:true to sde_get_type_dogma instead of memorizing attribute IDs 182/277 etc.
- \"How do I build / manufacture / produce X\" or \"bill of materials / production chain\" → sde_build_type FIRST (classifies the whole build tree + buy-vs-build gates), then sde_get_production_chain for quantities. Do NOT give fitting advice (modules/tank/DPS) for a build request unless the user explicitly asks about fitting.

Not in the SDE: market prices and fitted-yield simulation (stacking penalties). Compute yield/ISK math yourself from the dogma attributes the tools return.";

/// The predicates that can stand alone, and those that cannot. ADR 0003 requires
/// every text needing this distinction to **derive** from one statement of it
/// rather than restate it, or they drift the next time a predicate is added —
/// which is exactly what happened when `type_ids` moved sides (ADR Amendment 1).
///
/// Deliberately phrased as "can stand alone" rather than as produce-versus-narrow.
/// Since Amendment 1 the two are no longer the same partition: `type_ids` both
/// produces a candidate set *and* narrows one, so a taxonomy reading of this list
/// is now wrong where a can-it-be-the-only-predicate reading stays true.
pub(crate) const STANDALONE_PREDICATES: &str =
    "attribute {id, op?, value?}, group_ids, category_ids, type_ids";
pub(crate) const NARROW_ONLY_PREDICATES: &str = "meta_group_ids, published_only and query";

/// The narrow cases where a `sde_find_types` response invites a wrong conclusion.
/// Kept a free function so the branches are testable without a store, and so the
/// empty case and the truncated case cannot both fire — an empty result is never
/// truncated, so their conditions are disjoint by construction.
///
/// `recorded` is how many Types record an ExplicitValue for the attribute at all,
/// counted before this call's operator and narrowing predicates. An empty answer
/// means two different things either side of it, and only one of them is about the
/// attribute: `recorded == 0` says the SDE stores nothing for it, while
/// `recorded > 0` says the caller's own predicates excluded every carrier. Reading
/// the second as the first is the mis-signal this whole tool exists to end, and
/// `total_matched` alone cannot tell them apart.
///
/// `anything_modifies` is a closure rather than a bool because it is only worth a
/// map lookup in the empty-attribute case.
pub(crate) fn guidance_for(
    attribute_id: Option<u64>,
    recorded: usize,
    total_matched: usize,
    truncated: bool,
    anything_modifies: impl Fn(u64) -> bool,
) -> Option<String> {
    if let Some(attr) = attribute_id
        && total_matched == 0
        && recorded > 0
    {
        // The count is the correction: it contradicts the "no data" reading on the
        // spot, and names the predicates as the thing to relax rather than the
        // attribute as the thing to doubt.
        let mut msg = format!(
            "{recorded} Types record an ExplicitValue for attribute {attr}, but none \
             of them satisfied the rest of this call — the attribute ID is not the \
             problem. Relax the predicates before concluding the SDE has no answer: \
             the op/value comparison first, then {NARROW_ONLY_PREDICATES}, then any \
             {STANDALONE_PREDICATES} you combined with it. Types with no ExplicitValue \
             are absent by design; they still HAVE the attribute at its DefaultValue, \
             which this tool never matches on."
        );
        if anything_modifies(attr) {
            msg.push_str(&format!(
                " Something in the SDE also MODIFIES this attribute, so if you meant \
                 'what boosts {attr}' rather than 'what carries {attr}', call \
                 sde_get_modifiers with attribute_id {attr}."
            ));
        }
        return Some(msg);
    }
    if let Some(attr) = attribute_id
        && total_matched == 0
    {
        // Naming the count is what makes this actionable rather than consoling: the
        // caller learns whether the other tool has an answer before spending a call.
        return Some(if anything_modifies(attr) {
            format!(
                "No Type records an ExplicitValue for attribute {attr}. Every Type \
                 still HAS it at its DefaultValue — this tool matches stored rows \
                 only. Something in the SDE does MODIFY this attribute, so if you \
                 meant 'what boosts {attr}' rather than 'what carries {attr}', call \
                 sde_get_modifiers with attribute_id {attr}."
            )
        } else {
            format!(
                "No Type records an ExplicitValue for attribute {attr}, and nothing \
                 in the SDE modifies it either. Every Type still HAS it at its \
                 DefaultValue — this tool matches stored rows only. Check the \
                 attribute is the one you meant with sde_search_dogma."
            )
        });
    }
    // Story 35: a call that succeeded, matched thousands, and came back cut. The
    // rollup below already is the narrowing axis; this says so rather than leaving
    // it to be inferred. Deliberately does not offer paging — offset pagination is
    // Out of Scope in #37, and suggesting it would send the caller after a
    // parameter that does not exist.
    truncated.then(|| {
        format!(
            "{total_matched} Types matched and only the first page is returned; \
             raising limit alone will not make this cheap. The `groups` rollup below \
             counts the full match set, not this page, so it is the axis to narrow \
             on: re-ask naming the Groups you want. Any of {STANDALONE_PREDICATES} \
             can carry the query; {NARROW_ONLY_PREDICATES} narrow it further, and \
             project_attributes keeps rows small when you do need many of them. \
             There is no offset — narrow the predicate rather than paging."
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guidance_separates_an_unrecorded_attribute_from_an_over_narrowed_call() {
        // The regression this pins: attribute 1971 is recorded by 66 Types, none of
        // them above 999. A post-filter zero told the caller "No Type records an
        // ExplicitValue for attribute 1971" — the SDE-has-no-data reading that
        // sde_find_types exists to end, produced by the tool itself.
        let g = guidance_for(Some(1971), 66, 0, false, |_| false).expect("empty answer");
        assert!(
            !g.contains("No Type records"),
            "must not deny the 66 stored rows: {g}"
        );
        assert!(g.contains("66 Types record"), "quotes the true count: {g}");
        assert!(
            g.contains("not the problem"),
            "acquits the attribute ID so the caller stops doubting it: {g}"
        );

        // With nothing recorded, the same zero means what the old text said.
        let g = guidance_for(Some(30), 0, 0, false, |_| false).expect("empty answer");
        assert!(g.contains("No Type records an ExplicitValue"), "{g}");
        assert!(
            g.contains("sde_search_dogma"),
            "routes to verification: {g}"
        );
    }

    #[test]
    fn guidance_keeps_the_modifiers_pointer_on_both_empty_shapes() {
        // HAS vs MODIFIES is live whichever way the answer emptied: a caller who
        // wanted "what boosts this" gets nothing from either count.
        let over_narrowed = guidance_for(Some(77), 5, 0, false, |_| true).expect("empty answer");
        assert!(
            over_narrowed.contains("sde_get_modifiers"),
            "{over_narrowed}"
        );
        let unrecorded = guidance_for(Some(77), 0, 0, false, |_| true).expect("empty answer");
        assert!(unrecorded.contains("sde_get_modifiers"), "{unrecorded}");

        // And stays silent about it when there is nothing on that side either.
        let neither = guidance_for(Some(77), 5, 0, false, |_| false).expect("empty answer");
        assert!(
            !neither.contains("sde_get_modifiers"),
            "must not route to a tool that is also empty: {neither}"
        );
    }

    #[test]
    fn guidance_is_absent_when_the_answer_is_neither_empty_nor_truncated() {
        assert!(guidance_for(Some(1971), 66, 66, false, |_| true).is_none());
        // A non-empty page is not the empty case, whatever the recorded count says.
        assert!(guidance_for(Some(1971), 66, 3, false, |_| true).is_none());
        // No attribute predicate, no attribute guidance.
        assert!(guidance_for(None, 0, 0, false, |_| true).is_none());
    }
}
