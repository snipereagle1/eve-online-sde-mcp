use std::{
    fs::File,
    io::{BufRead, BufReader, Seek, SeekFrom},
    path::Path,
};

use serde_json::Value;

use crate::store::SdeIndex;

pub fn fetch_by_id(index: &SdeIndex, id: u64) -> anyhow::Result<Value> {
    let &offset = index
        .id_index
        .get(&id)
        .ok_or_else(|| anyhow::anyhow!("ID {} not found", id))?;
    fetch_at_offset(&index.path, offset)
}

/// The records whose English name contains `query`: the ones named exactly that
/// first, then the rest, each half ascending by `_key` and the whole capped at
/// `limit`.
///
/// Collect → filter → sort → truncate, in that order, and each step is
/// load-bearing:
///
/// - `keep` sees the **whole** match set, before the cap. Filtering the returned
///   page instead under-fills it — a request for 10 published Types used to come
///   back with 3 while thousands matched, and nothing in the response said which.
/// - `keep` is handed a `_key`, not a record, so a predicate answered from an
///   in-memory index costs no seek. A predicate that needs the record itself does
///   not belong here.
/// - Exact names sort ahead of merely-containing ones because the callers that
///   pass `limit: 1` — `sde_get_solar_system` and `sde_get_region` by name — take
///   the first row as *the* answer. Ordering by ID alone made that row Mohas for
///   the query "Moh", and 74 other solar systems the same way: their names are
///   proper substrings of a lower-ID system's. Ranking is by name, never by ID
///   magnitude, so a longer name never wins by being older.
/// - The tiebreak is by ID rather than by `HashMap` iteration order, which is
///   stable within one process and varies between them: the same question used to
///   get a differently ordered answer after a restart.
pub fn search_by_name(
    index: &SdeIndex,
    query: &str,
    limit: usize,
    mut keep: impl FnMut(u64) -> bool,
) -> anyhow::Result<Vec<Value>> {
    let mut hits = index.name_index.ids_containing(query);
    hits.retain(|hit| keep(hit.id));
    hits.sort_unstable_by_key(|hit| (!hit.exact, hit.id));
    hits.truncate(limit);
    hits.iter().map(|hit| fetch_by_id(index, hit.id)).collect()
}

pub fn fetch_at_offset(path: &Path, offset: u64) -> anyhow::Result<Value> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut line = String::new();
    BufReader::new(file).read_line(&mut line)?;
    Ok(serde_json::from_str(line.trim())?)
}

/// Every language the SDE ships, per `translationLanguages.jsonl`. `is_localized`
/// requires a map's keys to be a subset of these, so a missing code makes every
/// real record look non-localized and silently disables `--language` entirely.
const LANG_CODES: &[&str] = &["en", "de", "es", "fr", "ja", "ko", "ru", "zh"];

pub fn apply_language_filter(value: &mut Value, lang: &str) {
    match value {
        Value::Object(map) => {
            if is_localized(map) {
                let chosen = map
                    .get(lang)
                    .or_else(|| map.get("en"))
                    .cloned()
                    .unwrap_or(Value::Null);
                *value = chosen;
            } else {
                for v in map.values_mut() {
                    apply_language_filter(v, lang);
                }
            }
        }
        Value::Array(arr) => {
            for v in arr {
                apply_language_filter(v, lang);
            }
        }
        _ => {}
    }
}

fn is_localized(map: &serde_json::Map<String, Value>) -> bool {
    !map.is_empty()
        && map.contains_key("en")
        && map.keys().all(|k| LANG_CODES.contains(&k.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_fixture(content: &str) -> (tempfile::NamedTempFile, std::path::PathBuf) {
        let mut f = tempfile::Builder::new()
            .suffix(".jsonl")
            .tempfile()
            .unwrap();
        f.write_all(content.as_bytes()).unwrap();
        let path = f.path().to_path_buf();
        (f, path)
    }

    #[test]
    fn fetch_by_id_returns_correct_record() {
        let fixture = "{\"_key\":34,\"name\":{\"en\":\"Tritanium\"}}\n{\"_key\":35,\"name\":{\"en\":\"Pyerite\"}}\n";
        let (_f, path) = write_fixture(fixture);
        let pb = indicatif::ProgressBar::hidden();
        let idx = crate::scan::scan_index_pub(&path, &pb).unwrap();

        let val = fetch_by_id(&idx, 34).unwrap();
        assert_eq!(val["_key"], 34);
        assert_eq!(val["name"]["en"], "Tritanium");
    }

    #[test]
    fn fetch_by_id_returns_error_for_missing_id() {
        let fixture = "{\"_key\":34,\"name\":{\"en\":\"Tritanium\"}}\n";
        let (_f, path) = write_fixture(fixture);
        let pb = indicatif::ProgressBar::hidden();
        let idx = crate::scan::scan_index_pub(&path, &pb).unwrap();

        assert!(fetch_by_id(&idx, 99).is_err());
    }

    #[test]
    fn search_by_name_finds_partial_match() {
        let fixture = "{\"_key\":34,\"name\":{\"en\":\"Tritanium\"}}\n{\"_key\":35,\"name\":{\"en\":\"Pyerite\"}}\n";
        let (_f, path) = write_fixture(fixture);
        let pb = indicatif::ProgressBar::hidden();
        let idx = crate::scan::scan_index_pub(&path, &pb).unwrap();

        let results = search_by_name(&idx, "trit", 10, |_| true).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["_key"], 34);
    }

    #[test]
    fn search_by_name_respects_limit() {
        let fixture = "{\"_key\":1,\"name\":{\"en\":\"Alpha\"}}\n{\"_key\":2,\"name\":{\"en\":\"Alpha Two\"}}\n{\"_key\":3,\"name\":{\"en\":\"Alpha Three\"}}\n";
        let (_f, path) = write_fixture(fixture);
        let pb = indicatif::ProgressBar::hidden();
        let idx = crate::scan::scan_index_pub(&path, &pb).unwrap();

        let results = search_by_name(&idx, "alpha", 2, |_| true).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn search_by_name_prefers_the_exact_name_over_a_lower_id_containing_one() {
        // The real shape: solar system Mohas (30000031) has a lower `_key` than Moh
        // (30000042) and contains its name. `sde_get_solar_system` asks for one row
        // and takes it, so ordering by ID alone answered "Moh" with Mohas — and with
        // 74 other systems the same way.
        let fixture = "{\"_key\":30000031,\"name\":{\"en\":\"Mohas\"}}\n{\"_key\":30000042,\"name\":{\"en\":\"Moh\"}}\n";
        let (_f, path) = write_fixture(fixture);
        let pb = indicatif::ProgressBar::hidden();
        let idx = crate::scan::scan_index_pub(&path, &pb).unwrap();

        let one = search_by_name(&idx, "Moh", 1, |_| true).unwrap();
        assert_eq!(one[0]["_key"], 30000042, "the system actually named Moh");

        // The containing matches are not dropped, only outranked — and they keep
        // their ID order behind the exact one.
        let all = search_by_name(&idx, "moh", 10, |_| true).unwrap();
        let keys: Vec<_> = all.iter().map(|v| v["_key"].as_u64().unwrap()).collect();
        assert_eq!(keys, vec![30000042, 30000031]);
    }

    #[test]
    fn search_by_name_ranks_by_name_not_by_id_magnitude() {
        // The exact match wins from the *back* of the ID order too: the rank is the
        // name, so an older containing record cannot take the row either way.
        let fixture =
            "{\"_key\":1,\"name\":{\"en\":\"Jan\"}}\n{\"_key\":2,\"name\":{\"en\":\"Janus\"}}\n";
        let (_f, path) = write_fixture(fixture);
        let pb = indicatif::ProgressBar::hidden();
        let idx = crate::scan::scan_index_pub(&path, &pb).unwrap();

        let one = search_by_name(&idx, "Jan", 1, |_| true).unwrap();
        assert_eq!(one[0]["_key"], 1);
    }

    #[test]
    fn search_by_name_keeps_every_record_sharing_the_exact_name() {
        // A name is not unique. Both carriers of an exact name outrank the merely
        // containing record, ascending by ID between themselves.
        let fixture = "{\"_key\":5,\"name\":{\"en\":\"Alpha Two\"}}\n{\"_key\":7,\"name\":{\"en\":\"Alpha\"}}\n{\"_key\":9,\"name\":{\"en\":\"Alpha\"}}\n";
        let (_f, path) = write_fixture(fixture);
        let pb = indicatif::ProgressBar::hidden();
        let idx = crate::scan::scan_index_pub(&path, &pb).unwrap();

        let all = search_by_name(&idx, "alpha", 10, |_| true).unwrap();
        let keys: Vec<_> = all.iter().map(|v| v["_key"].as_u64().unwrap()).collect();
        assert_eq!(keys, vec![7, 9, 5]);
    }

    #[test]
    fn apply_language_filter_replaces_localized_objects() {
        let mut val = serde_json::json!({
            "_key": 34,
            "name": {"en": "Tritanium", "de": "Tritanium", "fr": "Tritanium"}
        });
        apply_language_filter(&mut val, "de");
        assert_eq!(val["name"], "Tritanium");
    }

    #[test]
    fn apply_language_filter_falls_back_to_en() {
        let mut val = serde_json::json!({
            "name": {"en": "Tritanium", "de": "Tritanium"}
        });
        apply_language_filter(&mut val, "ja");
        assert_eq!(val["name"], "Tritanium");
    }

    #[test]
    fn apply_language_filter_handles_all_eight_sde_languages() {
        // The real SDE ships de/en/es/fr/ja/ko/ru/zh. A code missing from
        // LANG_CODES makes `is_localized` reject every production record, so the
        // filter turns into a no-op against real data while passing on any
        // fixture that happens to omit that language.
        let mut val = serde_json::json!({
            "name": {
                "de": "Mineralien", "en": "Mineral", "es": "Mineral",
                "fr": "Minéral", "ja": "無機物", "ko": "광물",
                "ru": "Минералы", "zh": "矿物"
            }
        });
        apply_language_filter(&mut val, "en");
        assert_eq!(val["name"], "Mineral");
    }

    #[test]
    fn apply_language_filter_skips_non_localized_objects() {
        let mut val = serde_json::json!({"typeID": 34, "quantity": 1});
        apply_language_filter(&mut val, "en");
        assert_eq!(val["typeID"], 34);
        assert_eq!(val["quantity"], 1);
    }
}
