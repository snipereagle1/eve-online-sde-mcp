//! Factions and NpcCorporations: the political entities the SDE names.

use rmcp::{
    ErrorData,
    handler::server::wrapper::Parameters,
    schemars::{self, JsonSchema},
    tool, tool_router,
};
use serde::Deserialize;

use crate::tools::SdeMcpServer;

// ── Parameter structs ────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
pub struct FactionIdParam {
    pub faction_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct CorporationIdParam {
    pub corporation_id: u64,
}

#[tool_router(router = politics_router, vis = "pub(crate)")]
impl SdeMcpServer {
    #[tool(description = "Get a faction by its faction ID")]
    async fn sde_get_faction(
        &self,
        Parameters(p): Parameters<FactionIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.factions, p.faction_id, "factions")
    }

    #[tool(description = "Get an NPC corporation by its corporation ID")]
    async fn sde_get_npc_corporation(
        &self,
        Parameters(p): Parameters<CorporationIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(
            &self.store.npc_corporations,
            p.corporation_id,
            "npcCorporations",
        )
    }
}

#[cfg(test)]
mod tests;
