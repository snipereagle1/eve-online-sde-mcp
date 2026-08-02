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
        name_index: crate::store::NameIndex::default(),
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
    // types.jsonl goes through the real `scan_types`, not the generic index
    // scan: `meta_group_of` reads the `type_meta_group` map that pass builds, so
    // a store wired by hand here would answer every `metaGroupID` question with
    // "absent" and quietly turn every fixture into a Tech I item.
    let (f1, types_path) = write_fixture(types_jsonl);
    let types = crate::scan::scan_types_pub(&types_path, &indicatif::ProgressBar::hidden())
        .expect("scan fixture types");
    let (f2, groups) = index(groups_jsonl);
    let (f3, blueprints) = index(blueprints_jsonl);
    let store = SdeStore {
        data_dir: std::path::PathBuf::from("/tmp"),
        build: 1,
        release_date: "2024-01-01".into(),
        files_scanned: 0,
        last_updated: "2024-01-01".into(),
        types: types.index,
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
        attribute_types: HashMap::new(),
        type_group: types.type_group,
        group_types: types.group_types,
        type_meta_group: types.type_meta_group,
        category_groups: HashMap::new(),
        published_types: types.published_types,
        dogma_attribute_text: Vec::new(),
        dogma_effect_text: Vec::new(),
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
