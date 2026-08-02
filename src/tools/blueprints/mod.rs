//! Blueprints: the recipe records, and the reverse map from a product back to the
//! blueprint that makes it.
//!
//! Reading a blueprint is one lookup and lives here; planning a whole build tree
//! out of many is `manufacturing`.

use rmcp::{
    ErrorData,
    handler::server::wrapper::Parameters,
    schemars::{self, JsonSchema},
    tool, tool_router,
};
use serde::Deserialize;

use crate::tools::SdeMcpServer;
use crate::tools::query;

// ── Parameter structs ────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
pub struct BlueprintTypeIdParam {
    pub blueprint_type_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct ProductTypeIdParam {
    pub product_type_id: u64,
}

#[tool_router(router = blueprints_router, vis = "pub(crate)")]
impl SdeMcpServer {
    #[tool(description = "Get a blueprint by its blueprint type ID")]
    async fn sde_get_blueprint(
        &self,
        Parameters(p): Parameters<BlueprintTypeIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.blueprints, p.blueprint_type_id, "blueprints")
    }

    #[tool(
        description = "Get the blueprint that produces a given product type, tagged with the activity that makes it. Returns {\"blueprint\": {...}, \"activity\": \"manufacturing\"|\"reaction\"} — the activity tells you whether the product is manufactured or comes out of a reaction (the two are distinct production paths with different rules; reactions ignore material efficiency). {\"result\": null} means the product has no blueprint at all (a raw material you must buy/mine). For a full multi-tier bill of materials, prefer sde_build_type."
    )]
    async fn sde_get_blueprint_for_product(
        &self,
        Parameters(p): Parameters<ProductTypeIdParam>,
    ) -> Result<String, ErrorData> {
        let Some(&bp_ref) = self.store.product_to_blueprint.get(&p.product_type_id) else {
            return Ok(serde_json::json!({"result": null}).to_string());
        };
        let mut blueprint = query::fetch_by_id(&self.store.blueprints, bp_ref.blueprint_id)
            .map_err(|_| {
                ErrorData::internal_error(
                    format!("blueprint {} missing from index", bp_ref.blueprint_id),
                    None,
                )
            })?;
        self.filter(&mut blueprint);
        Ok(serde_json::json!({
            "blueprint": blueprint,
            "activity": bp_ref.activity.as_str(),
        })
        .to_string())
    }
}

#[cfg(test)]
mod tests;
