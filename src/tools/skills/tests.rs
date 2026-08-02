use std::sync::Arc;

use super::*;
use crate::store::SdeStore;
use crate::tools::testkit::{default_store, make_index};

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
    let fixture_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sde");
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
    dogma.push_str("{\"_key\":115,\"dogmaAttributes\":[{\"attributeID\":275,\"value\":1.0}]}\n");
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

/// The MCP seam: a real scan of `tests/fixtures/sde`, a real `SdeMcpServer`,
/// and a real MCP client talking to it over an in-memory duplex transport.
/// Every test here drives a tool the way a client does — over the wire, not
/// by calling the handler method directly.
mod mcp_seam {
    use crate::tools::testkit::*;

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
}
