use std::{collections::HashMap, sync::Arc};

use rmcp::handler::server::wrapper::Parameters;

use super::*;
use crate::store::SdeStore;
use crate::tools::testkit::{default_store, make_index, make_server};

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
    let (_a, dogma_attributes) = make_index("{\"_key\":77,\"name\":{\"en\":\"miningAmount\"}}\n");
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

/// Tool tests driven over the wire — see [`crate::tools::testkit::Seam`].
mod mcp_seam {
    use crate::tools::testkit::*;

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
    async fn get_modifiers_points_at_the_selector_when_nothing_modifies() -> anyhow::Result<()> {
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
    async fn search_dogma_returns_attributes_and_effects_as_distinct_lists() -> anyhow::Result<()> {
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
    async fn search_dogma_caps_each_list_separately_and_reports_the_totals() -> anyhow::Result<()> {
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
