use std::sync::Arc;

use rmcp::handler::server::wrapper::Parameters;

use super::*;
use crate::store::SdeStore;
use crate::tools::testkit::{default_store, make_blueprint_index, make_server};

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

/// The MCP seam: a real scan of `tests/fixtures/sde`, a real `SdeMcpServer`,
/// and a real MCP client talking to it over an in-memory duplex transport.
/// Every test here drives a tool the way a client does — over the wire, not
/// by calling the handler method directly.
mod mcp_seam {
    use crate::tools::testkit::*;

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
}
