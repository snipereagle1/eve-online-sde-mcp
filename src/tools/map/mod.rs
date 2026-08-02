//! The map: SolarSystems, Constellations, Regions, NpcStations, and the stargate
//! graph a route is found over.
//!
//! NpcStation lives here rather than with the political entities that own it —
//! it belongs to a SolarSystem, and that is how an agent reaches one. See ADR
//! 0004.

use std::{collections::HashMap, sync::Arc};

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

#[tool_router(router = map_router, vis = "pub(crate)")]
impl SdeMcpServer {
    #[tool(description = "Get a solar system by ID or name")]
    async fn sde_get_solar_system(
        &self,
        Parameters(p): Parameters<SolarSystemParam>,
    ) -> Result<String, ErrorData> {
        match (p.system_id, p.name) {
            (Some(id), _) => {
                self.fetch_filtered(&self.store.map_solar_systems, id, "mapSolarSystems")
            }
            (None, Some(name)) => {
                let results = self.search_filtered(&self.store.map_solar_systems, &name, 1)?;
                results
                    .into_iter()
                    .next()
                    .map(|v| serde_json::to_string(&v).unwrap())
                    .ok_or_else(|| {
                        ErrorData::invalid_params(format!("Solar system '{name}' not found"), None)
                    })
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
                    .ok_or_else(|| {
                        ErrorData::invalid_params(format!("Region '{name}' not found"), None)
                    })
            }
            (None, None) => Err(ErrorData::invalid_params("Provide region_id or name", None)),
        }
    }

    #[tool(description = "Get a constellation by its constellation ID")]
    async fn sde_get_constellation(
        &self,
        Parameters(p): Parameters<ConstellationIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(
            &self.store.map_constellations,
            p.constellation_id,
            "mapConstellations",
        )
    }

    #[tool(description = "Get an NPC station by its station ID")]
    async fn sde_get_npc_station(
        &self,
        Parameters(p): Parameters<StationIdParam>,
    ) -> Result<String, ErrorData> {
        self.fetch_filtered(&self.store.npc_stations, p.station_id, "npcStations")
    }

    #[tool(
        description = "Find the shortest route between two solar systems; returns jump count and system ID path"
    )]
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
}

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

#[cfg(test)]
mod tests;
