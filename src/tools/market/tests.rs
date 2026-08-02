use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;

use super::*;
use crate::store::SdeStore;
use crate::tools::testkit::{default_store, make_index, make_server};

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

/// The MCP seam: a real scan of `tests/fixtures/sde`, a real `SdeMcpServer`,
/// and a real MCP client talking to it over an in-memory duplex transport.
/// Every test here drives a tool the way a client does — over the wire, not
/// by calling the handler method directly.
mod mcp_seam {
    use crate::tools::testkit::*;

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
}
