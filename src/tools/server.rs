use std::sync::Arc;

use rmcp::{
    ErrorData, ServerHandler,
    model::{Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};

use super::guidance::SERVER_INSTRUCTIONS;
use super::query;
use super::query::pick_name;
use crate::store::SdeStore;

// ── Server ───────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct SdeMcpServer {
    pub store: Arc<SdeStore>,
    pub language: Option<String>,
}

impl SdeMcpServer {
    pub fn new(store: Arc<SdeStore>, language: Option<String>) -> Self {
        Self { store, language }
    }

    pub(crate) fn filter(&self, value: &mut serde_json::Value) {
        if let Some(ref lang) = self.language {
            query::apply_language_filter(value, lang);
        }
    }

    pub(crate) fn fetch_filtered(
        &self,
        index: &crate::store::SdeIndex,
        id: u64,
        label: &str,
    ) -> Result<String, ErrorData> {
        let mut val = query::fetch_by_id(index, id).map_err(|_| {
            ErrorData::invalid_params(format!("ID {id} not found in {label}"), None)
        })?;
        self.filter(&mut val);
        Ok(serde_json::to_string(&val).unwrap())
    }

    pub(crate) fn search_filtered(
        &self,
        index: &crate::store::SdeIndex,
        q: &str,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>, ErrorData> {
        self.search_filtered_where(index, q, limit, |_| true)
    }

    /// As [`Self::search_filtered`], but keeping only the records whose `_key`
    /// satisfies `keep`. The predicate runs over the whole match set before the
    /// limit — see [`query::search_by_name`] — so a narrowed search still returns a
    /// full page when one exists.
    pub(crate) fn search_filtered_where(
        &self,
        index: &crate::store::SdeIndex,
        q: &str,
        limit: usize,
        keep: impl FnMut(u64) -> bool,
    ) -> Result<Vec<serde_json::Value>, ErrorData> {
        let mut results = query::search_by_name(index, q, limit, keep)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        for v in &mut results {
            self.filter(v);
        }
        Ok(results)
    }

    /// English (or configured-language) name of a type, or None if unknown.
    pub(crate) fn type_name(&self, id: u64) -> Option<String> {
        let v = query::fetch_by_id(&self.store.types, id).ok()?;
        pick_name(v.get("name"), self.language.as_deref())
    }

    /// Name of a dogma attribute by id, or None.
    pub(crate) fn attribute_name(&self, id: u64) -> Option<String> {
        let v = query::fetch_by_id(&self.store.dogma_attributes, id).ok()?;
        pick_name(v.get("name"), self.language.as_deref())
    }

    /// True if a type is a skill (category 16), via type→group→category. Used by the
    /// levers view to float skills above implants/boosters/ships when listing what
    /// modifies an attribute — the skill sources are what a training plan cares about.
    pub(crate) fn is_skill(&self, type_id: u64) -> bool {
        let Ok(t) = query::fetch_by_id(&self.store.types, type_id) else {
            return false;
        };
        let Some(group_id) = t.get("groupID").and_then(|x| x.as_u64()) else {
            return false;
        };
        query::fetch_by_id(&self.store.groups, group_id)
            .ok()
            .and_then(|g| g.get("categoryID").and_then(|x| x.as_u64()))
            == Some(16)
    }
}

// ── Tool implementations ─────────────────────────────────────────────────────

#[tool_router(router = server_router, vis = "pub(crate)")]
impl SdeMcpServer {
    #[tool(
        description = "Get SDE metadata: build number, release date, data directory, files scanned"
    )]
    async fn sde_status(&self) -> String {
        serde_json::to_string(&serde_json::json!({
            "build": self.store.build,
            "release_date": self.store.release_date,
            "data_dir": self.store.data_dir.display().to_string(),
            "files_scanned": self.store.files_scanned,
            "last_updated": self.store.last_updated,
        }))
        .unwrap()
    }
}

/// The one router the handler dispatches on, summed from each domain's. A new
/// domain module is the only thing that changes this function — a new tool inside
/// an existing one composes automatically.
impl SdeMcpServer {
    fn tool_router() -> rmcp::handler::server::router::tool::ToolRouter<Self> {
        Self::server_router()
            + Self::blueprints_router()
            + Self::dogma_router()
            + Self::types_router()
            + Self::manufacturing_router()
            + Self::map_router()
            + Self::market_router()
            + Self::politics_router()
            + Self::skills_router()
    }
}

#[tool_handler(name = "eve-sde-mcp", version = "0.1.0")]
impl ServerHandler for SdeMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "eve-sde-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(SERVER_INSTRUCTIONS)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use crate::tools::testkit::make_server;

    #[tokio::test]
    async fn sde_status_returns_build_metadata() {
        let server = make_server();
        let result = server.sde_status().await;
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["build"], 42);
        assert_eq!(v["release_date"], "2024-01-01");
        assert_eq!(v["files_scanned"], 17);
    }

    /// The agent-visible contract: every tool's name, description and input schema,
    /// as a client sees them over `tools/list`. Descriptions are prompt-critical —
    /// they are what an agent routes on — so a reworded one is a behavioral change
    /// even though no code path moved, and a dropped router is invisible to every
    /// other test here. Regenerate deliberately with
    /// `SDE_UPDATE_TOOLS_LIST=1 cargo test tools_list_matches_the_pinned_contract`
    /// when a tool is genuinely added or changed.
    #[tokio::test]
    async fn tools_list_matches_the_pinned_contract() -> anyhow::Result<()> {
        let seam = crate::tools::testkit::Seam::serving(make_server().store, None).await?;
        let mut tools = seam.client.list_all_tools().await?;
        seam.shutdown().await?;

        // Sorted by name so the snapshot does not encode router composition order,
        // which is an implementation detail; `serde_json::Map` is a `BTreeMap` here,
        // so every nested key order is already canonical.
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        let actual = serde_json::to_string_pretty(&tools)? + "\n";

        let golden =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tools-list.json");
        // Exactly "1", never mere presence: a stray `SDE_UPDATE_TOOLS_LIST=0` left
        // exported in a shell would otherwise turn this guard into a rubber stamp.
        // The rewrite still fails the test, so a regenerating run is never green and
        // the new golden has to be read and committed deliberately.
        if std::env::var("SDE_UPDATE_TOOLS_LIST").as_deref() == Ok("1") {
            std::fs::write(&golden, &actual)?;
            anyhow::bail!("rewrote {} — review and commit it", golden.display());
        }
        let expected = std::fs::read_to_string(&golden)?;
        assert_eq!(
            actual,
            expected,
            "the MCP tool contract drifted from {}",
            golden.display()
        );
        Ok(())
    }

    /// The other half of what a client reads before it routes: the server
    /// instructions. They are prompt-critical for the same reason tool
    /// descriptions are, and nothing else asserts they reach the wire.
    #[tokio::test]
    async fn initialize_carries_the_server_instructions() -> anyhow::Result<()> {
        let seam = crate::tools::testkit::Seam::serving(make_server().store, None).await?;
        let instructions = seam.client.peer_info().and_then(|i| i.instructions.clone());
        seam.shutdown().await?;
        assert_eq!(
            instructions.as_deref(),
            Some(crate::tools::guidance::SERVER_INSTRUCTIONS)
        );
        Ok(())
    }

    /// Tool tests driven over the wire — see [`crate::tools::testkit::Seam`].
    mod mcp_seam {
        use crate::tools::testkit::Seam;

        #[tokio::test]
        async fn status_reports_the_scanned_build() -> anyhow::Result<()> {
            let seam = Seam::boot().await?;
            let r = seam.call("sde_status", serde_json::json!({})).await?;
            assert_eq!(r["build"], 3333874);
            assert_eq!(r["release_date"], "2024-01-15");
            assert!(r["files_scanned"].as_u64().unwrap() > 0);
            seam.shutdown().await
        }
    }
}
