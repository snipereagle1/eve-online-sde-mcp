//! Shared test harness for every `tools::*` domain's tests.
//!
//! Two ways to drive the server, and both are needed. The `make_*` helpers build a
//! `SdeStore` over tempfile JSONL fixtures and call handler methods directly, which
//! is how a single tool's branches are tested cheaply. [`Seam`] boots a real
//! `SdeMcpServer` over an in-memory duplex transport and talks to it as an MCP
//! client does — over the wire, not by calling the handler — which is what pins the
//! serialized shape a client actually receives.

use std::{
    collections::{HashMap, HashSet},
    io::Write as _,
    sync::Arc,
};

use rmcp::{
    ClientHandler, RoleClient, ServiceExt as _,
    model::{CallToolRequestParams, CallToolResult, ClientInfo},
    service::RunningService,
};

use crate::store::SdeStore;
use crate::tools::SdeMcpServer;

pub(crate) fn write_fixture(content: &str) -> (tempfile::NamedTempFile, std::path::PathBuf) {
    let mut f = tempfile::Builder::new()
        .suffix(".jsonl")
        .tempfile()
        .unwrap();
    f.write_all(content.as_bytes()).unwrap();
    let path = f.path().to_path_buf();
    (f, path)
}

pub(crate) fn make_index(content: &str) -> (tempfile::NamedTempFile, crate::store::SdeIndex) {
    let (_f, path) = write_fixture(content);
    let pb = indicatif::ProgressBar::hidden();
    let idx = crate::scan::scan_index_pub(&path, &pb).unwrap();
    (_f, idx)
}

pub(crate) fn make_blueprint_index(
    content: &str,
) -> (
    tempfile::NamedTempFile,
    crate::store::SdeIndex,
    HashMap<u64, crate::store::BlueprintRef>,
) {
    let (f, path) = write_fixture(content);
    let pb = indicatif::ProgressBar::hidden();
    let (idx, p2b) = crate::scan::scan_blueprints_pub(&path, &pb).unwrap();
    (f, idx, p2b)
}

pub(crate) fn make_server() -> SdeMcpServer {
    SdeMcpServer::new(Arc::new(default_store()), None)
}

pub(crate) fn empty_index() -> crate::store::SdeIndex {
    crate::store::SdeIndex {
        path: std::path::PathBuf::from("/dev/null"),
        id_index: HashMap::new(),
        name_index: crate::store::NameIndex::default(),
    }
}

pub(crate) fn default_store() -> SdeStore {
    SdeStore {
        data_dir: std::path::PathBuf::from("/tmp"),
        build: 42,
        release_date: "2024-01-01".to_string(),
        files_scanned: 17,
        last_updated: "2024-01-01".to_string(),
        types: empty_index(),
        groups: empty_index(),
        categories: empty_index(),
        blueprints: empty_index(),
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
        product_to_blueprint: HashMap::new(),
        stargate_graph: HashMap::new(),
        attribute_modifiers: HashMap::new(),
        effect_to_types: HashMap::new(),
        attribute_types: HashMap::new(),
        type_group: HashMap::new(),
        group_types: HashMap::new(),
        type_meta_group: HashMap::new(),
        category_groups: HashMap::new(),
        published_types: HashSet::new(),
        dogma_attribute_text: Vec::new(),
        dogma_effect_text: Vec::new(),
    }
}

#[derive(Clone, Default)]
pub(crate) struct DummyClient;

impl ClientHandler for DummyClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo::default()
    }
}

/// A booted client/server pair: a real scan of `tests/fixtures/sde`, a real
/// `SdeMcpServer`, and a real MCP client talking to it over an in-memory duplex
/// transport. A test driving a tool through this is exercising it the way a client
/// does — over the wire, not by calling the handler method directly — which is why
/// each domain's tests keep a `mod mcp_seam` for the ones that do.
///
/// Call [`Seam::shutdown`] at the end of a test to cancel the client and join the
/// server task.
pub(crate) struct Seam {
    pub(crate) client: RunningService<RoleClient, DummyClient>,
    server: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Seam {
    /// Scan the JSONL fixtures and serve them to a live client.
    pub(crate) async fn boot() -> anyhow::Result<Self> {
        Self::boot_with_language(Some("en".to_string())).await
    }

    /// As [`Seam::boot`], but with an explicit server language — `None` is the
    /// default all-languages mode, where localized fields come back as full
    /// eight-language maps.
    pub(crate) async fn boot_with_language(language: Option<String>) -> anyhow::Result<Self> {
        let fixture_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sde");
        let store = crate::scan::scan_sde(&fixture_dir, 3333874, "2024-01-15")?;

        Self::serving(store, language).await
    }

    /// Serve an already-built store — for the tool-contract snapshot, which needs
    /// no data at all and should not pay for a fixture scan.
    pub(crate) async fn serving(
        store: Arc<SdeStore>,
        language: Option<String>,
    ) -> anyhow::Result<Self> {
        let (server_transport, client_transport) = tokio::io::duplex(65536);
        let server = tokio::spawn(async move {
            SdeMcpServer::new(store, language)
                .serve(server_transport)
                .await?
                .waiting()
                .await?;
            anyhow::Ok(())
        });
        let client = DummyClient.serve(client_transport).await?;
        Ok(Self { client, server })
    }

    /// Call `tool` and parse its single text content block as JSON.
    pub(crate) async fn call(
        &self,
        tool: &str,
        args: serde_json::Value,
    ) -> anyhow::Result<serde_json::Value> {
        let result = self.try_call(tool, args).await?;
        let text = result
            .content
            .first()
            .and_then(|c| c.raw.as_text())
            .map(|t| t.text.as_str())
            .expect("expected text content");
        Ok(serde_json::from_str(text).expect("invalid JSON in tool response"))
    }

    /// Call `tool` without interpreting the result — for asserting that a call
    /// fails.
    pub(crate) async fn try_call(
        &self,
        tool: &str,
        args: serde_json::Value,
    ) -> anyhow::Result<CallToolResult> {
        let mut request = CallToolRequestParams::new(tool.to_string());
        if let Some(map) = args.as_object().filter(|m| !m.is_empty()) {
            request = request.with_arguments(map.clone());
        }
        Ok(self.client.call_tool(request).await?)
    }

    pub(crate) async fn shutdown(self) -> anyhow::Result<()> {
        self.client.cancel().await?;
        let _ = self.server.await;
        Ok(())
    }
}

/// The `type_id`s of a `sde_find_types` answer, in the order returned.
pub(crate) fn ids_of(response: &serde_json::Value) -> Vec<u64> {
    response["types"]
        .as_array()
        .expect("types array")
        .iter()
        .map(|t| t["type_id"].as_u64().expect("type_id"))
        .collect()
}

/// The `_key`s of a name-search answer — a bare array of whole records — in the
/// order returned.
pub(crate) fn keys_of(response: &serde_json::Value) -> Vec<u64> {
    response
        .as_array()
        .expect("array of records")
        .iter()
        .map(|t| t["_key"].as_u64().expect("_key"))
        .collect()
}
