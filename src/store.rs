use std::collections::HashMap;
use std::path::PathBuf;

#[allow(dead_code)]
pub struct SdeIndex {
    pub path: PathBuf,
    pub id_index: HashMap<u64, u64>,
    pub name_index: HashMap<String, u64>,
}

#[allow(dead_code)]
pub struct SdeStore {
    pub data_dir: PathBuf,
    pub build: u64,
    pub release_date: String,
    pub files_scanned: usize,
    pub last_updated: String,
    pub types: SdeIndex,
    pub groups: SdeIndex,
    pub categories: SdeIndex,
    pub blueprints: SdeIndex,
    pub type_materials: SdeIndex,
    pub map_solar_systems: SdeIndex,
    pub map_constellations: SdeIndex,
    pub map_regions: SdeIndex,
    pub map_stargates: SdeIndex,
    pub npc_stations: SdeIndex,
    pub market_groups: SdeIndex,
    pub dogma_attributes: SdeIndex,
    pub dogma_effects: SdeIndex,
    pub factions: SdeIndex,
    pub npc_corporations: SdeIndex,
    pub skins: SdeIndex,
    pub product_to_blueprint: HashMap<u64, u64>,
    pub stargate_graph: HashMap<u64, Vec<u64>>,
}
