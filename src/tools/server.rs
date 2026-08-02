use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use rmcp::{
    ErrorData, ServerHandler,
    handler::server::wrapper::Parameters,
    model::{Implementation, ServerCapabilities, ServerInfo},
    schemars::{self, JsonSchema},
    tool, tool_handler, tool_router,
};
use serde::Deserialize;

use super::guidance::SERVER_INSTRUCTIONS;
use super::manufacturing;
use super::query;
use super::query::pick_name;
use crate::store::SdeStore;

// ── Parameter structs ────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
pub struct BuildTypeParam {
    /// The Type ID you want to manufacture/build (a ship, module, component, etc.)
    pub product_type_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct ProductionChainParam {
    /// The Type ID you want to build.
    pub product_type_id: u64,
    /// Number of runs (units, when output-per-run is 1) of the target to build. Default 1.
    pub runs: Option<u64>,
    /// Which decomposable origins to build rather than buy: any of "manufactured",
    /// "reaction-output". Defaults to both (build the whole tree). Anything not built
    /// lands in the shopping list.
    pub build_origins: Option<Vec<String>>,
    /// Force these Type IDs to be bought even when their origin is being built
    /// (e.g. buy fuel blocks instead of decomposing them into ice + PI).
    pub buy_type_ids: Option<Vec<u64>>,
    /// Default material efficiency (%) applied to every manufacturing job. Default 0.
    /// Reactions always ignore ME.
    pub me: Option<i64>,
    /// Per-Type material-efficiency overrides (%), keyed by Type ID; overrides `me`
    /// for those types only.
    pub me_overrides: Option<HashMap<u64, i64>>,
}

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

    #[tool(
        description = "Plan how to manufacture / build / produce a Type (ship, module, component, …): the FIRST tool to call for 'how do I build X', 'what do I need to make X', 'bill of materials', or 'production chain'. Classifies the whole build tree and returns: whether the target is buildable (and its material-efficiency mode), the distinct decomposable origins present (manufactured vs reaction-output), per-origin buy-vs-build decision gates (each input tagged with its origin, ME mode, and required skills), the aggregate blueprint-job skills across the chain, and any out-of-scope leaves (invention or planetary-industry items you must buy). This is the classify-only router — neutral facts, no recommendations. Once the player picks what to build vs buy, call sde_get_production_chain for the resolved quantities and shopping list."
    )]
    async fn sde_build_type(
        &self,
        Parameters(p): Parameters<BuildTypeParam>,
    ) -> Result<String, ErrorData> {
        let store = Arc::clone(&self.store);
        let lang = self.language.clone();
        let target_id = p.product_type_id;
        let result = tokio::task::spawn_blocking(move || {
            manufacturing::build_type(&store, target_id, lang.as_deref())
        })
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
        .map_err(|e| ErrorData::invalid_params(e, None))?;
        Ok(serde_json::to_string(&result).unwrap())
    }

    #[tool(
        description = "Compute the resolved production chain for a build: given the player's buy-vs-build decisions, returns per-Type build jobs (runs, output-per-run, leftover from run-rounding) and one consolidated shopping list grouped by origin (minerals, moon materials, PI, etc.), plus the aggregate job skills. Material efficiency reduces manufacturing material cost (floored at one unit per run); reactions ignore ME. Shared intermediates are counted once across the whole tree before run-rounding. Decisions: build_origins toggles which decomposable origins to build (default: build everything), buy_type_ids force-buys specific Types (e.g. fuel blocks), me / me_overrides set material efficiency. Call sde_build_type first to discover the decision gates."
    )]
    async fn sde_get_production_chain(
        &self,
        Parameters(p): Parameters<ProductionChainParam>,
    ) -> Result<String, ErrorData> {
        let build_origins: HashSet<manufacturing::Origin> = match p.build_origins {
            Some(keys) => {
                let mut set = HashSet::new();
                for key in keys {
                    let origin = manufacturing::Origin::from_key(&key).ok_or_else(|| {
                        ErrorData::invalid_params(
                            format!(
                                "unknown build_origin '{key}' (expected 'manufactured' or 'reaction-output')"
                            ),
                            None,
                        )
                    })?;
                    set.insert(origin);
                }
                set
            }
            None => HashSet::from([
                manufacturing::Origin::Manufactured,
                manufacturing::Origin::ReactionOutput,
            ]),
        };

        let params = manufacturing::ChainParams {
            target_id: p.product_type_id,
            runs: p.runs.unwrap_or(1),
            build_origins,
            buy_type_ids: p.buy_type_ids.unwrap_or_default().into_iter().collect(),
            me_default: p.me.unwrap_or(0),
            me_overrides: p.me_overrides.unwrap_or_default(),
        };

        let store = Arc::clone(&self.store);
        let lang = self.language.clone();
        let result = tokio::task::spawn_blocking(move || {
            manufacturing::production_chain(&store, &params, lang.as_deref())
        })
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
        .map_err(|e| ErrorData::invalid_params(e, None))?;
        Ok(serde_json::to_string(&result).unwrap())
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

// ── sde_find_types ───────────────────────────────────────────────────────────

// ── sde_search_dogma ─────────────────────────────────────────────────────────

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

    #[tokio::test]
    async fn mcp_handshake_initialize_and_list_tools() -> anyhow::Result<()> {
        let seam = crate::tools::testkit::Seam::serving(make_server().store, None).await?;
        let tools = seam.client.list_all_tools().await?;
        assert!(tools.len() >= 28, "expected ≥28 tools, got {}", tools.len());
        let names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert!(names.contains(&"sde_status"));
        assert!(names.contains(&"sde_find_route"));
        assert!(names.contains(&"sde_get_market_group_tree"));
        assert!(names.contains(&"sde_get_type_dogma"));
        assert!(names.contains(&"sde_get_skill_plan"));
        assert!(names.contains(&"sde_get_modifiers"));
        assert!(names.contains(&"sde_get_types"));
        assert!(names.contains(&"sde_get_types_dogma"));
        assert!(names.contains(&"sde_resolve_types"));
        assert!(names.contains(&"sde_get_skill_sp"));
        seam.shutdown().await
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
        if std::env::var_os("SDE_UPDATE_TOOLS_LIST").is_some() {
            std::fs::write(&golden, &actual)?;
            return Ok(());
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

    /// The MCP seam: a real scan of `tests/fixtures/sde`, a real `SdeMcpServer`,
    /// and a real MCP client talking to it over an in-memory duplex transport.
    /// Every test here drives a tool the way a client does — over the wire, not
    /// by calling the handler method directly.
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
