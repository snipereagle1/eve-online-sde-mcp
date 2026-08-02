//! MarketGroups: the browsable tree CCP files Types under for the in-game market.

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
pub struct MarketGroupIdParam {
    pub market_group_id: u64,
}

#[tool_router(router = market_router, vis = "pub(crate)")]
impl SdeMcpServer {
    #[tool(description = "Get a market group by its market group ID")]
    async fn sde_get_market_group(
        &self,
        Parameters(p): Parameters<MarketGroupIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.market_groups, p.market_group_id, "marketGroups")
    }

    #[tool(
        description = "Get the full ancestor chain for a market group, from root to the given group"
    )]
    async fn sde_get_market_group_tree(
        &self,
        Parameters(p): Parameters<MarketGroupIdParam>,
    ) -> Result<String, ErrorData> {
        const MAX_HOPS: usize = 20;
        let mut chain = Vec::new();
        let mut id = p.market_group_id;
        loop {
            if chain.len() >= MAX_HOPS {
                return Err(ErrorData::internal_error(
                    "Market group chain exceeds 20 hops",
                    None,
                ));
            }
            let mut val = query::fetch_by_id(&self.store.market_groups, id).map_err(|_| {
                ErrorData::invalid_params(format!("ID {id} not found in marketGroups"), None)
            })?;
            self.filter(&mut val);
            let parent = val.get("parentGroupID").and_then(|v| v.as_u64());
            chain.push(val);
            match parent {
                Some(pid) => id = pid,
                None => break,
            }
        }
        chain.reverse();
        Ok(serde_json::to_string(&chain).unwrap())
    }
}

#[cfg(test)]
mod tests;
