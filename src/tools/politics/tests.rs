use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;

use super::*;
use crate::store::SdeStore;
use crate::tools::testkit::{default_store, make_index};

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
    let (_f, npc_corporations) =
        make_index("{\"_key\":1000035,\"name\":{\"en\":\"Caldari Navy\"},\"factionID\":500001}\n");
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

/// The MCP seam: a real scan of `tests/fixtures/sde`, a real `SdeMcpServer`,
/// and a real MCP client talking to it over an in-memory duplex transport.
/// Every test here drives a tool the way a client does — over the wire, not
/// by calling the handler method directly.
mod mcp_seam {
    use crate::tools::testkit::*;

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
}
