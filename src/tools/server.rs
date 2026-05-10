use std::{collections::HashMap, sync::Arc};

use rmcp::{
    ServerHandler,
    handler::server::wrapper::Parameters,
    model::{Implementation, ServerCapabilities, ServerInfo},
    schemars::{self, JsonSchema},
    tool, tool_handler, tool_router,
    ErrorData,
};
use serde::Deserialize;

use crate::store::SdeStore;
use super::query;

// ── Parameter structs ────────────────────────────────────────────────────────

#[derive(Deserialize, JsonSchema)]
pub struct TypeIdParam {
    /// EVE type ID
    pub type_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchTypesParam {
    /// Name substring to search for (case-insensitive)
    pub query: String,
    /// Maximum results to return (default: 10)
    pub limit: Option<u64>,
    /// Only return published types
    pub published_only: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
pub struct GroupIdParam {
    pub group_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct CategoryIdParam {
    pub category_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct BlueprintTypeIdParam {
    pub blueprint_type_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct ProductTypeIdParam {
    pub product_type_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct SolarSystemParam {
    /// Solar system ID (provide this or name)
    pub system_id: Option<u64>,
    /// Solar system name (provide this or system_id)
    pub name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct SearchParam {
    pub query: String,
    pub limit: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
pub struct RegionParam {
    pub region_id: Option<u64>,
    pub name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
pub struct ConstellationIdParam {
    pub constellation_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct StationIdParam {
    pub station_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct RouteParam {
    pub from_system_id: u64,
    pub to_system_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct MarketGroupIdParam {
    pub market_group_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct AttributeIdParam {
    pub attribute_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct EffectIdParam {
    pub effect_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct FactionIdParam {
    pub faction_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct CorporationIdParam {
    pub corporation_id: u64,
}

#[derive(Deserialize, JsonSchema)]
pub struct SkinIdParam {
    pub skin_id: u64,
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

    fn filter(&self, value: &mut serde_json::Value) {
        if let Some(ref lang) = self.language {
            query::apply_language_filter(value, lang);
        }
    }

    fn fetch_filtered(
        &self,
        index: &crate::store::SdeIndex,
        id: u64,
        label: &str,
    ) -> Result<String, ErrorData> {
        let mut val = query::fetch_by_id(index, id)
            .map_err(|_| ErrorData::invalid_params(format!("ID {id} not found in {label}"), None))?;
        self.filter(&mut val);
        Ok(serde_json::to_string(&val).unwrap())
    }

    fn search_filtered(
        &self,
        index: &crate::store::SdeIndex,
        q: &str,
        limit: usize,
    ) -> Result<Vec<serde_json::Value>, ErrorData> {
        let mut results = query::search_by_name(index, q, limit)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        for v in &mut results {
            self.filter(v);
        }
        Ok(results)
    }
}

// ── Tool implementations ─────────────────────────────────────────────────────

#[tool_router]
impl SdeMcpServer {
    #[tool(description = "Get SDE metadata: build number, release date, data directory, files scanned")]
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

    #[tool(description = "Get a type (item) by its type ID")]
    async fn sde_get_type(
        &self,
        Parameters(p): Parameters<TypeIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.types, p.type_id, "types")
    }

    #[tool(description = "Search types by name substring; optionally filter to published only")]
    async fn sde_search_types(
        &self,
        Parameters(p): Parameters<SearchTypesParam>,
    ) -> Result<String, ErrorData> {
        let limit = p.limit.unwrap_or(10) as usize;
        let published_only = p.published_only.unwrap_or(false);
        let mut results = self.search_filtered(&self.store.types, &p.query, limit)?;
        if published_only {
            results.retain(|v| v.get("published").and_then(|b| b.as_bool()).unwrap_or(false));
        }
        Ok(serde_json::to_string(&results).unwrap())
    }

    #[tool(description = "Get a type group by its group ID")]
    async fn sde_get_group(
        &self,
        Parameters(p): Parameters<GroupIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.groups, p.group_id, "groups")
    }

    #[tool(description = "Get a type category by its category ID")]
    async fn sde_get_category(
        &self,
        Parameters(p): Parameters<CategoryIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.categories, p.category_id, "categories")
    }

    #[tool(description = "Get the materials required to reprocess a type")]
    async fn sde_get_type_materials(
        &self,
        Parameters(p): Parameters<TypeIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.type_materials, p.type_id, "typeMaterials")
    }

    #[tool(description = "Get a blueprint by its blueprint type ID")]
    async fn sde_get_blueprint(
        &self,
        Parameters(p): Parameters<BlueprintTypeIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.blueprints, p.blueprint_type_id, "blueprints")
    }

    #[tool(description = "Get the blueprint that manufactures a given product type")]
    async fn sde_get_blueprint_for_product(
        &self,
        Parameters(p): Parameters<ProductTypeIdParam>,
    ) -> Result<String, ErrorData> {
        let Some(&bp_id) = self.store.product_to_blueprint.get(&p.product_type_id) else {
            return Ok(serde_json::json!({"result": null}).to_string());
        };
        self.fetch_filtered(&self.store.blueprints, bp_id, "blueprints")
    }

    #[tool(description = "Get a solar system by ID or name")]
    async fn sde_get_solar_system(
        &self,
        Parameters(p): Parameters<SolarSystemParam>,
    ) -> Result<String, ErrorData> {
        match (p.system_id, p.name) {
            (Some(id), _) => self.fetch_filtered(&self.store.map_solar_systems, id, "mapSolarSystems"),
            (None, Some(name)) => {
                let results = self.search_filtered(&self.store.map_solar_systems, &name, 1)?;
                results
                    .into_iter()
                    .next()
                    .map(|v| serde_json::to_string(&v).unwrap())
                    .ok_or_else(|| ErrorData::invalid_params(format!("Solar system '{name}' not found"), None))
            }
            (None, None) => Err(ErrorData::invalid_params("Provide system_id or name", None)),
        }
    }

    #[tool(description = "Search solar systems by name substring")]
    async fn sde_search_solar_systems(
        &self,
        Parameters(p): Parameters<SearchParam>,
    ) -> Result<String, ErrorData> {
        let limit = p.limit.unwrap_or(10) as usize;
        let results = self.search_filtered(&self.store.map_solar_systems, &p.query, limit)?;
        Ok(serde_json::to_string(&results).unwrap())
    }

    #[tool(description = "Get a region by ID or name")]
    async fn sde_get_region(
        &self,
        Parameters(p): Parameters<RegionParam>,
    ) -> Result<String, ErrorData> {
        match (p.region_id, p.name) {
            (Some(id), _) => self.fetch_filtered(&self.store.map_regions, id, "mapRegions"),
            (None, Some(name)) => {
                let results = self.search_filtered(&self.store.map_regions, &name, 1)?;
                results
                    .into_iter()
                    .next()
                    .map(|v| serde_json::to_string(&v).unwrap())
                    .ok_or_else(|| ErrorData::invalid_params(format!("Region '{name}' not found"), None))
            }
            (None, None) => Err(ErrorData::invalid_params("Provide region_id or name", None)),
        }
    }

    #[tool(description = "Get a constellation by its constellation ID")]
    async fn sde_get_constellation(
        &self,
        Parameters(p): Parameters<ConstellationIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.map_constellations, p.constellation_id, "mapConstellations")
    }

    #[tool(description = "Get an NPC station by its station ID")]
    async fn sde_get_npc_station(
        &self,
        Parameters(p): Parameters<StationIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.npc_stations, p.station_id, "npcStations")
    }

    #[tool(description = "Find the shortest route between two solar systems; returns jump count and system ID path")]
    async fn sde_find_route(
        &self,
        Parameters(p): Parameters<RouteParam>,
    ) -> Result<String, ErrorData> {
        let store = Arc::clone(&self.store);
        let from = p.from_system_id;
        let to = p.to_system_id;
        let path = tokio::task::spawn_blocking(move || bfs_route(&store.stargate_graph, from, to))
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        match path {
            Some(p) => Ok(serde_json::to_string(&serde_json::json!({
                "jumps": p.len().saturating_sub(1),
                "path": p,
            }))
            .unwrap()),
            None => Err(ErrorData::invalid_params("No route found", None)),
        }
    }

    #[tool(description = "Get a market group by its market group ID")]
    async fn sde_get_market_group(
        &self,
        Parameters(p): Parameters<MarketGroupIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.market_groups, p.market_group_id, "marketGroups")
    }

    #[tool(description = "Get the full ancestor chain for a market group, from root to the given group")]
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

    #[tool(description = "Get a dogma attribute by its attribute ID")]
    async fn sde_get_dogma_attribute(
        &self,
        Parameters(p): Parameters<AttributeIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.dogma_attributes, p.attribute_id, "dogmaAttributes")
    }

    #[tool(description = "Get a dogma effect by its effect ID")]
    async fn sde_get_dogma_effect(
        &self,
        Parameters(p): Parameters<EffectIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.dogma_effects, p.effect_id, "dogmaEffects")
    }

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
        self.fetch_filtered(&self.store.npc_corporations, p.corporation_id, "npcCorporations")
    }

    #[tool(description = "Get a SKIN (ship SKINs) by its skin ID")]
    async fn sde_get_skin(
        &self,
        Parameters(p): Parameters<SkinIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.skins, p.skin_id, "skins")
    }
}

#[tool_handler(name = "eve-sde-mcp", version = "0.1.0", instructions = "EVE Online Static Data Export MCP server")]
impl ServerHandler for SdeMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "eve-sde-mcp",
                env!("CARGO_PKG_VERSION"),
            ))
    }
}

// ── BFS route ────────────────────────────────────────────────────────────────

fn bfs_route(graph: &HashMap<u64, Vec<u64>>, from: u64, to: u64) -> Option<Vec<u64>> {
    if from == to {
        return Some(vec![from]);
    }
    let mut queue = std::collections::VecDeque::new();
    let mut prev: HashMap<u64, u64> = HashMap::new();
    queue.push_back(from);
    prev.insert(from, from);
    while let Some(curr) = queue.pop_front() {
        if let Some(neighbors) = graph.get(&curr) {
            for &next in neighbors {
                if let std::collections::hash_map::Entry::Vacant(e) = prev.entry(next) {
                    e.insert(curr);
                    if next == to {
                        let mut path = vec![to];
                        let mut node = to;
                        while node != from {
                            node = prev[&node];
                            path.push(node);
                        }
                        path.reverse();
                        return Some(path);
                    }
                    queue.push_back(next);
                }
            }
        }
    }
    None
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn write_fixture(content: &str) -> (tempfile::NamedTempFile, std::path::PathBuf) {
        let mut f = tempfile::Builder::new()
            .suffix(".jsonl")
            .tempfile()
            .unwrap();
        f.write_all(content.as_bytes()).unwrap();
        let path = f.path().to_path_buf();
        (f, path)
    }

    fn make_index(content: &str) -> (tempfile::NamedTempFile, crate::store::SdeIndex) {
        let (_f, path) = write_fixture(content);
        let pb = indicatif::ProgressBar::hidden();
        let idx = crate::scan::scan_index_pub(&path, &pb).unwrap();
        (_f, idx)
    }

    fn make_server() -> SdeMcpServer {
        SdeMcpServer::new(Arc::new(default_store()), None)
    }

    fn empty_index() -> crate::store::SdeIndex {
        crate::store::SdeIndex {
            path: std::path::PathBuf::from("/dev/null"),
            id_index: HashMap::new(),
            name_index: HashMap::new(),
        }
    }

    fn default_store() -> SdeStore {
        SdeStore {
            data_dir: std::path::PathBuf::from("/tmp"),
            build: 42,
            release_date: "2024-01-01".to_string(),
            files_scanned: 16,
            last_updated: "2024-01-01".to_string(),
            types: empty_index(),
            groups: empty_index(),
            categories: empty_index(),
            blueprints: empty_index(),
            type_materials: empty_index(),
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
        }
    }

    #[tokio::test]
    async fn sde_status_returns_build_metadata() {
        let server = make_server();
        let result = server.sde_status().await;
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["build"], 42);
        assert_eq!(v["release_date"], "2024-01-01");
        assert_eq!(v["files_scanned"], 16);
    }

    #[tokio::test]
    async fn sde_get_type_returns_error_for_missing_id() {
        let server = make_server();
        let result = server
            .sde_get_type(Parameters(TypeIdParam { type_id: 99 }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("99"));
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
    async fn sde_get_type_returns_record_for_known_id() {
        let (_f, types) = make_index(
            "{\"_key\":34,\"name\":{\"en\":\"Tritanium\"},\"published\":true}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                types,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_type(Parameters(TypeIdParam { type_id: 34 }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 34);
    }

    #[tokio::test]
    async fn sde_search_types_returns_matches() {
        let (_f, types) = make_index(
            "{\"_key\":34,\"name\":{\"en\":\"Tritanium\"},\"published\":true}\n\
             {\"_key\":35,\"name\":{\"en\":\"Pyerite\"},\"published\":false}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                types,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_search_types(Parameters(SearchTypesParam {
                query: "trit".to_string(),
                limit: None,
                published_only: None,
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0]["_key"], 34);
    }

    #[tokio::test]
    async fn sde_search_types_published_only_filters_unpublished() {
        let (_f, types) = make_index(
            "{\"_key\":34,\"name\":{\"en\":\"Tritanium\"},\"published\":true}\n\
             {\"_key\":35,\"name\":{\"en\":\"Tritan Scrap\"},\"published\":false}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                types,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_search_types(Parameters(SearchTypesParam {
                query: "tritan".to_string(),
                limit: None,
                published_only: Some(true),
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0]["_key"], 34);
    }

    #[tokio::test]
    async fn sde_get_group_returns_record_for_known_id() {
        let (_f, groups) =
            make_index("{\"_key\":18,\"name\":{\"en\":\"Mineral\"},\"categoryID\":4}\n");
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                groups,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_group(Parameters(GroupIdParam { group_id: 18 }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 18);
        assert_eq!(v["categoryID"], 4);
    }

    #[tokio::test]
    async fn sde_get_group_returns_error_for_missing_id() {
        let server = make_server();
        let result = server
            .sde_get_group(Parameters(GroupIdParam { group_id: 99 }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("99"));
    }

    #[tokio::test]
    async fn sde_get_category_returns_record_for_known_id() {
        let (_f, categories) =
            make_index("{\"_key\":4,\"name\":{\"en\":\"Material\"},\"published\":true}\n");
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                categories,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_category(Parameters(CategoryIdParam { category_id: 4 }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 4);
    }

    #[tokio::test]
    async fn sde_get_category_returns_error_for_missing_id() {
        let server = make_server();
        let result = server
            .sde_get_category(Parameters(CategoryIdParam { category_id: 99 }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("99"));
    }

    #[tokio::test]
    async fn sde_get_type_materials_returns_record_for_known_id() {
        let (_f, type_materials) = make_index(
            "{\"_key\":34,\"materials\":[{\"typeID\":35,\"quantity\":10}]}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                type_materials,
                ..default_store()
            }),
            None,
        );
        let result = server
            .sde_get_type_materials(Parameters(TypeIdParam { type_id: 34 }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 34);
        assert_eq!(v["materials"][0]["typeID"], 35);
    }

    #[tokio::test]
    async fn sde_get_type_materials_returns_error_for_missing_id() {
        let server = make_server();
        let result = server
            .sde_get_type_materials(Parameters(TypeIdParam { type_id: 99 }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("99"));
    }

    #[tokio::test]
    async fn mcp_handshake_initialize_and_list_tools() -> anyhow::Result<()> {
        use rmcp::{ClientHandler, ServiceExt as _, model::ClientInfo};

        #[derive(Clone, Default)]
        struct DummyClient;
        impl ClientHandler for DummyClient {
            fn get_info(&self) -> ClientInfo {
                ClientInfo::default()
            }
        }

        let (server_transport, client_transport) = tokio::io::duplex(65536);
        let store = make_server().store;
        let server_handle = tokio::spawn(async move {
            SdeMcpServer::new(store, None)
                .serve(server_transport)
                .await?
                .waiting()
                .await?;
            anyhow::Ok(())
        });
        let client = DummyClient.serve(client_transport).await?;
        let tools = client.list_all_tools().await?;
        assert!(
            tools.len() >= 21,
            "expected ≥21 tools, got {}",
            tools.len()
        );
        let names: Vec<_> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert!(names.contains(&"sde_status"));
        assert!(names.contains(&"sde_find_route"));
        assert!(names.contains(&"sde_get_market_group_tree"));
        client.cancel().await?;
        let _ = server_handle.await;
        Ok(())
    }

    #[tokio::test]
    async fn sde_get_solar_system_by_id_returns_record() {
        let (_f, map_solar_systems) = make_index(
            "{\"_key\":30000142,\"name\":{\"en\":\"Jita\"},\"securityStatus\":0.9459}\n",
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
        let (_f, map_solar_systems) = make_index(
            "{\"_key\":30000142,\"name\":{\"en\":\"Jita\"},\"securityStatus\":0.9459}\n",
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
                name: Some("Jita".to_string()),
            }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 30000142);
    }

    #[tokio::test]
    async fn sde_find_route_returns_path_with_correct_jump_count() {
        // A → B → C → D: 3 jumps, 4 systems
        let mut graph = HashMap::new();
        graph.insert(1u64, vec![2u64]);
        graph.insert(2u64, vec![1u64, 3u64]);
        graph.insert(3u64, vec![2u64, 4u64]);
        graph.insert(4u64, vec![3u64]);
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                stargate_graph: graph,
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
        // system 99 is isolated
        let server = SdeMcpServer::new(
            Arc::new(SdeStore {
                stargate_graph: graph,
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

    fn make_blueprint_index(
        content: &str,
    ) -> (tempfile::NamedTempFile, crate::store::SdeIndex, HashMap<u64, u64>) {
        let mut f = tempfile::Builder::new()
            .suffix(".jsonl")
            .tempfile()
            .unwrap();
        use std::io::Write as _;
        f.write_all(content.as_bytes()).unwrap();
        let path = f.path().to_path_buf();
        let pb = indicatif::ProgressBar::hidden();
        let (idx, p2b) = crate::scan::scan_blueprints_pub(&path, &pb).unwrap();
        (f, idx, p2b)
    }

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
        assert_eq!(v["_key"], 683);
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

    #[tokio::test]
    async fn sde_get_market_group_returns_record_for_known_id() {
        let (_f, market_groups) = make_index(
            "{\"_key\":4,\"name\":{\"en\":\"Ships\"},\"parentGroupID\":null}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore { market_groups, ..default_store() }),
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
            .sde_get_market_group(Parameters(MarketGroupIdParam { market_group_id: 99 }))
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
            Arc::new(SdeStore { market_groups, ..default_store() }),
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
        let (_f, market_groups) =
            make_index("{\"_key\":1,\"name\":{\"en\":\"Root\"}}\n");
        let server = SdeMcpServer::new(
            Arc::new(SdeStore { market_groups, ..default_store() }),
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

    #[tokio::test]
    async fn sde_get_dogma_attribute_returns_record_for_known_id() {
        let (_f, dogma_attributes) = make_index(
            "{\"_key\":37,\"name\":{\"en\":\"CPU\"},\"unitID\":5}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore { dogma_attributes, ..default_store() }),
            None,
        );
        let result = server
            .sde_get_dogma_attribute(Parameters(AttributeIdParam { attribute_id: 37 }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 37);
        assert_eq!(v["unitID"], 5);
    }

    #[tokio::test]
    async fn sde_get_dogma_attribute_returns_error_for_missing_id() {
        let server = make_server();
        let result = server
            .sde_get_dogma_attribute(Parameters(AttributeIdParam { attribute_id: 99 }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("99"));
    }

    #[tokio::test]
    async fn sde_get_dogma_effect_returns_record_for_known_id() {
        let (_f, dogma_effects) = make_index(
            "{\"_key\":11,\"name\":{\"en\":\"loPower\"},\"effectCategory\":0}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore { dogma_effects, ..default_store() }),
            None,
        );
        let result = server
            .sde_get_dogma_effect(Parameters(EffectIdParam { effect_id: 11 }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 11);
    }

    #[tokio::test]
    async fn sde_get_faction_returns_record_for_known_id() {
        let (_f, factions) = make_index(
            "{\"_key\":500001,\"name\":{\"en\":\"Caldari State\"},\"corporationID\":1000035}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore { factions, ..default_store() }),
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
        let (_f, npc_corporations) = make_index(
            "{\"_key\":1000035,\"name\":{\"en\":\"Caldari Navy\"},\"factionID\":500001}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore { npc_corporations, ..default_store() }),
            None,
        );
        let result = server
            .sde_get_npc_corporation(Parameters(CorporationIdParam { corporation_id: 1000035 }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 1000035);
        assert_eq!(v["factionID"], 500001);
    }

    #[tokio::test]
    async fn sde_get_skin_returns_record_for_known_id() {
        let (_f, skins) = make_index(
            "{\"_key\":1001,\"name\":{\"en\":\"Caldari Navy SKIN\"},\"typeID\":638}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore { skins, ..default_store() }),
            None,
        );
        let result = server
            .sde_get_skin(Parameters(SkinIdParam { skin_id: 1001 }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 1001);
        assert_eq!(v["typeID"], 638);
    }

    #[tokio::test]
    async fn sde_get_skin_returns_error_for_missing_id() {
        let server = make_server();
        let result = server
            .sde_get_skin(Parameters(SkinIdParam { skin_id: 99 }))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("99"));
    }

    #[tokio::test]
    async fn sde_search_solar_systems_returns_matches() {
        let (_f, map_solar_systems) = make_index(
            "{\"_key\":30000142,\"name\":{\"en\":\"Jita\"},\"securityStatus\":0.9459}\n\
             {\"_key\":30000144,\"name\":{\"en\":\"Perimeter\"},\"securityStatus\":0.9531}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore { map_solar_systems, ..default_store() }),
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
        let (_f, map_regions) =
            make_index("{\"_key\":10000002,\"name\":{\"en\":\"The Forge\"}}\n");
        let server = SdeMcpServer::new(
            Arc::new(SdeStore { map_regions, ..default_store() }),
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
        let (_f, map_constellations) = make_index(
            "{\"_key\":20000020,\"name\":{\"en\":\"Kimotoro\"},\"regionID\":10000002}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore { map_constellations, ..default_store() }),
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
        let (_f, npc_stations) = make_index(
            "{\"_key\":60003760,\"solarSystemID\":30000142,\"ownerID\":1000035}\n",
        );
        let server = SdeMcpServer::new(
            Arc::new(SdeStore { npc_stations, ..default_store() }),
            None,
        );
        let result = server
            .sde_get_npc_station(Parameters(StationIdParam { station_id: 60003760 }))
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["_key"], 60003760);
    }

    #[tokio::test]
    async fn mcp_all_21_tools_via_fixture_data() -> anyhow::Result<()> {
        use rmcp::{ClientHandler, ServiceExt as _, model::{ClientInfo, CallToolRequestParams}};

        #[derive(Clone, Default)]
        struct DummyClient;
        impl ClientHandler for DummyClient {
            fn get_info(&self) -> ClientInfo {
                ClientInfo::default()
            }
        }

        fn text_json(result: &rmcp::model::CallToolResult) -> serde_json::Value {
            let text = result
                .content
                .first()
                .and_then(|c| c.raw.as_text())
                .map(|t| t.text.as_str())
                .expect("expected text content");
            serde_json::from_str(text).expect("invalid JSON in tool response")
        }

        fn obj(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
            v.as_object().unwrap().clone()
        }

        let fixture_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/sde");
        let store = crate::scan::scan_sde(&fixture_dir, 3333874, "2024-01-15")?;

        let (server_transport, client_transport) = tokio::io::duplex(65536);
        let server_handle = tokio::spawn(async move {
            SdeMcpServer::new(store, Some("en".to_string()))
                .serve(server_transport)
                .await?
                .waiting()
                .await?;
            anyhow::Ok(())
        });
        let client = DummyClient.serve(client_transport).await?;

        // sde_status
        let r = text_json(&client.call_tool(CallToolRequestParams::new("sde_status")).await?);
        assert_eq!(r["build"], 3333874);
        assert_eq!(r["release_date"], "2024-01-15");
        assert!(r["files_scanned"].as_u64().unwrap() > 0);

        // sde_get_type
        let r = text_json(&client.call_tool(
            CallToolRequestParams::new("sde_get_type").with_arguments(obj(serde_json::json!({"type_id": 34}))),
        ).await?);
        assert_eq!(r["_key"], 34);
        assert_eq!(r["name"], "Tritanium");

        // sde_search_types
        let r = text_json(&client.call_tool(
            CallToolRequestParams::new("sde_search_types")
                .with_arguments(obj(serde_json::json!({"query": "trit"}))),
        ).await?);
        assert!(r.as_array().unwrap().iter().any(|v| v["_key"] == 34));

        // sde_get_group
        let r = text_json(&client.call_tool(
            CallToolRequestParams::new("sde_get_group").with_arguments(obj(serde_json::json!({"group_id": 18}))),
        ).await?);
        assert_eq!(r["_key"], 18);
        assert_eq!(r["name"], "Mineral");

        // sde_get_category
        let r = text_json(&client.call_tool(
            CallToolRequestParams::new("sde_get_category")
                .with_arguments(obj(serde_json::json!({"category_id": 4}))),
        ).await?);
        assert_eq!(r["_key"], 4);
        assert_eq!(r["name"], "Material");

        // sde_get_type_materials
        let r = text_json(&client.call_tool(
            CallToolRequestParams::new("sde_get_type_materials")
                .with_arguments(obj(serde_json::json!({"type_id": 1230}))),
        ).await?);
        assert_eq!(r["_key"], 1230);
        assert!(r["materials"].as_array().is_some());

        // sde_get_blueprint
        let r = text_json(&client.call_tool(
            CallToolRequestParams::new("sde_get_blueprint")
                .with_arguments(obj(serde_json::json!({"blueprint_type_id": 16228}))),
        ).await?);
        assert_eq!(r["_key"], 16228);
        assert!(r["activities"]["manufacturing"].is_object());

        // sde_get_blueprint_for_product (Ferox blueprint makes Ferox)
        let r = text_json(&client.call_tool(
            CallToolRequestParams::new("sde_get_blueprint_for_product")
                .with_arguments(obj(serde_json::json!({"product_type_id": 16227}))),
        ).await?);
        assert_eq!(r["_key"], 16228);

        // sde_get_solar_system (by ID)
        let r = text_json(&client.call_tool(
            CallToolRequestParams::new("sde_get_solar_system")
                .with_arguments(obj(serde_json::json!({"system_id": 30000142}))),
        ).await?);
        assert_eq!(r["_key"], 30000142);
        assert_eq!(r["name"], "Jita");

        // sde_search_solar_systems
        let r = text_json(&client.call_tool(
            CallToolRequestParams::new("sde_search_solar_systems")
                .with_arguments(obj(serde_json::json!({"query": "jita"}))),
        ).await?);
        assert!(r.as_array().unwrap().iter().any(|v| v["_key"] == 30000142));

        // sde_get_region (by ID)
        let r = text_json(&client.call_tool(
            CallToolRequestParams::new("sde_get_region")
                .with_arguments(obj(serde_json::json!({"region_id": 10000002}))),
        ).await?);
        assert_eq!(r["_key"], 10000002);
        assert_eq!(r["name"], "The Forge");

        // sde_get_constellation
        let r = text_json(&client.call_tool(
            CallToolRequestParams::new("sde_get_constellation")
                .with_arguments(obj(serde_json::json!({"constellation_id": 20000020}))),
        ).await?);
        assert_eq!(r["_key"], 20000020);
        assert_eq!(r["name"], "Kimotoro");

        // sde_get_npc_station
        let r = text_json(&client.call_tool(
            CallToolRequestParams::new("sde_get_npc_station")
                .with_arguments(obj(serde_json::json!({"station_id": 60003760}))),
        ).await?);
        assert_eq!(r["_key"], 60003760);
        assert_eq!(r["solarSystemID"], 30000142);

        // sde_find_route: Jita → Perimeter (1 jump)
        let r = text_json(&client.call_tool(
            CallToolRequestParams::new("sde_find_route").with_arguments(obj(serde_json::json!({
                "from_system_id": 30000142,
                "to_system_id": 30000144,
            }))),
        ).await?);
        assert_eq!(r["jumps"], 1);
        assert_eq!(r["path"].as_array().unwrap().len(), 2);
        assert_eq!(r["path"][0], 30000142);
        assert_eq!(r["path"][1], 30000144);

        // sde_find_route: unreachable system → error response (Ikuchi has no stargates)
        let err = client.call_tool(
            CallToolRequestParams::new("sde_find_route").with_arguments(obj(serde_json::json!({
                "from_system_id": 30000142,
                "to_system_id": 30000138,
            }))),
        ).await;
        assert!(err.is_err(), "expected error for unreachable system");

        // sde_get_market_group
        let r = text_json(&client.call_tool(
            CallToolRequestParams::new("sde_get_market_group")
                .with_arguments(obj(serde_json::json!({"market_group_id": 1857}))),
        ).await?);
        assert_eq!(r["_key"], 1857);
        assert_eq!(r["name"], "Minerals");

        // sde_get_market_group_tree (Minerals → Materials → Manufacture & Research)
        let r = text_json(&client.call_tool(
            CallToolRequestParams::new("sde_get_market_group_tree")
                .with_arguments(obj(serde_json::json!({"market_group_id": 1857}))),
        ).await?);
        let arr = r.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["_key"], 475); // root: Manufacture & Research
        assert_eq!(arr[2]["_key"], 1857); // leaf: Minerals

        // sde_get_dogma_attribute
        let r = text_json(&client.call_tool(
            CallToolRequestParams::new("sde_get_dogma_attribute")
                .with_arguments(obj(serde_json::json!({"attribute_id": 30}))),
        ).await?);
        assert_eq!(r["_key"], 30);
        assert_eq!(r["name"], "power");

        // sde_get_dogma_effect
        let r = text_json(&client.call_tool(
            CallToolRequestParams::new("sde_get_dogma_effect")
                .with_arguments(obj(serde_json::json!({"effect_id": 11}))),
        ).await?);
        assert_eq!(r["_key"], 11);
        assert_eq!(r["name"], "loPower");

        // sde_get_faction
        let r = text_json(&client.call_tool(
            CallToolRequestParams::new("sde_get_faction")
                .with_arguments(obj(serde_json::json!({"faction_id": 500001}))),
        ).await?);
        assert_eq!(r["_key"], 500001);
        assert_eq!(r["name"], "Caldari State");

        // sde_get_npc_corporation
        let r = text_json(&client.call_tool(
            CallToolRequestParams::new("sde_get_npc_corporation")
                .with_arguments(obj(serde_json::json!({"corporation_id": 1000035}))),
        ).await?);
        assert_eq!(r["_key"], 1000035);
        assert_eq!(r["name"], "Caldari Navy");

        // sde_get_skin
        let r = text_json(&client.call_tool(
            CallToolRequestParams::new("sde_get_skin")
                .with_arguments(obj(serde_json::json!({"skin_id": 50}))),
        ).await?);
        assert_eq!(r["_key"], 50);
        assert_eq!(r["internalName"], "Ferox Caldari Union Day YC124");

        client.cancel().await?;
        let _ = server_handle.await;
        Ok(())
    }
}
