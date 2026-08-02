use std::{collections::HashMap, sync::Arc};

use rmcp::handler::server::wrapper::Parameters;

use super::*;
use crate::store::SdeStore;
use crate::tools::testkit::{default_store, make_index};

/// A minimal `mapSolarSystems.jsonl` declaring just the given IDs, so a route test
/// can build a synthetic stargate graph whose endpoints the server accepts as real.
fn systems_jsonl(ids: &[u64]) -> String {
    ids.iter()
        .map(|id| format!(r#"{{"_key":{id},"name":{{"en":"System {id}"}}}}"#))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
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
async fn sde_get_solar_system_by_id_returns_record() {
    let (_f, map_solar_systems) =
        make_index("{\"_key\":30000142,\"name\":{\"en\":\"Jita\"},\"securityStatus\":0.9459}\n");
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
    let (_f, map_solar_systems) =
        make_index("{\"_key\":30000142,\"name\":{\"en\":\"Jita\"},\"securityStatus\":0.9459}\n");
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
async fn sde_find_route_returns_path_with_correct_jump_count() {
    // A → B → C → D: 3 jumps, 4 systems
    let mut graph = HashMap::new();
    graph.insert(1u64, vec![2u64]);
    graph.insert(2u64, vec![1u64, 3u64]);
    graph.insert(3u64, vec![2u64, 4u64]);
    graph.insert(4u64, vec![3u64]);
    let (_f, systems) = make_index(&systems_jsonl(&[1, 2, 3, 4]));
    let server = SdeMcpServer::new(
        Arc::new(SdeStore {
            stargate_graph: graph,
            map_solar_systems: systems,
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
    // system 99 is a declared system with no stargates, so it is reachable from
    // nowhere — distinct from an ID the SDE never declared, which errors earlier.
    let (_f, systems) = make_index(&systems_jsonl(&[1, 2, 99]));
    let server = SdeMcpServer::new(
        Arc::new(SdeStore {
            stargate_graph: graph,
            map_solar_systems: systems,
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
    let (_f, map_constellations) =
        make_index("{\"_key\":20000020,\"name\":{\"en\":\"Kimotoro\"},\"regionID\":10000002}\n");
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

/// Tool tests driven over the wire — see [`crate::tools::testkit::Seam`].
mod mcp_seam {
    use crate::tools::testkit::*;

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
    async fn search_solar_systems_orders_by_id_across_separate_processes() -> anyhow::Result<()> {
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
}

#[tokio::test]
async fn sde_find_route_rejects_a_system_id_the_sde_does_not_declare() {
    let (_f, systems) = make_index(&systems_jsonl(&[1, 2]));
    let server = SdeMcpServer::new(
        Arc::new(SdeStore {
            stargate_graph: HashMap::from([(1u64, vec![2u64]), (2u64, vec![1u64])]),
            map_solar_systems: systems,
            ..default_store()
        }),
        None,
    );
    // Same ID for both endpoints: the trivial-route short-circuit must not answer
    // ahead of the check, or a typo comes back as a confident zero-jump route.
    let err = server
        .sde_find_route(Parameters(RouteParam {
            from_system_id: 30000149,
            to_system_id: 30000149,
        }))
        .await
        .unwrap_err();
    assert!(
        err.message.contains("30000149 not found"),
        "got {}",
        err.message
    );
}

#[tokio::test]
async fn sde_find_route_returns_zero_jumps_for_a_declared_system_to_itself() {
    let (_f, systems) = make_index(&systems_jsonl(&[1, 2]));
    let server = SdeMcpServer::new(
        Arc::new(SdeStore {
            stargate_graph: HashMap::from([(1u64, vec![2u64]), (2u64, vec![1u64])]),
            map_solar_systems: systems,
            ..default_store()
        }),
        None,
    );
    let result = server
        .sde_find_route(Parameters(RouteParam {
            from_system_id: 1,
            to_system_id: 1,
        }))
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(v["jumps"], 0);
    assert_eq!(v["path"].as_array().unwrap().len(), 1);
}
