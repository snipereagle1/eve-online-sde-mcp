use std::collections::HashMap;
use std::path::PathBuf;

pub(crate) struct SdeIndex {
    pub(crate) path: PathBuf,
    pub(crate) id_index: HashMap<u64, u64>,
    pub(crate) name_index: HashMap<String, u64>,
}

pub(crate) struct SdeStore {
    pub(crate) data_dir: PathBuf,
    pub(crate) build: u64,
    pub(crate) release_date: String,
    pub(crate) files_scanned: usize,
    pub(crate) last_updated: String,
    pub(crate) types: SdeIndex,
    pub(crate) groups: SdeIndex,
    pub(crate) categories: SdeIndex,
    pub(crate) blueprints: SdeIndex,
    pub(crate) type_materials: SdeIndex,
    pub(crate) map_solar_systems: SdeIndex,
    pub(crate) map_constellations: SdeIndex,
    pub(crate) map_regions: SdeIndex,
    pub(crate) npc_stations: SdeIndex,
    pub(crate) market_groups: SdeIndex,
    pub(crate) dogma_attributes: SdeIndex,
    pub(crate) dogma_effects: SdeIndex,
    pub(crate) factions: SdeIndex,
    pub(crate) npc_corporations: SdeIndex,
    pub(crate) skins: SdeIndex,
    pub(crate) product_to_blueprint: HashMap<u64, u64>,
    pub(crate) stargate_graph: HashMap<u64, Vec<u64>>,
}
