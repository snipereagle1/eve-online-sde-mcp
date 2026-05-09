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

pub fn search_by_name(index: &SdeIndex, query: &str, limit: usize) -> anyhow::Result<Vec<Value>> {
    let q = query.to_lowercase();
    let mut results = Vec::new();
    for (name, &offset) in &index.name_index {
        if name.contains(&q) {
            results.push(fetch_at_offset(&index.path, offset)?);
            if results.len() >= limit {
                break;
            }
        }
    }
    Ok(results)
}

pub fn fetch_at_offset(path: &Path, offset: u64) -> anyhow::Result<Value> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut line = String::new();
    BufReader::new(file).read_line(&mut line)?;
    Ok(serde_json::from_str(line.trim())?)
}

const LANG_CODES: &[&str] = &["en", "de", "fr", "ja", "ko", "ru", "zh"];

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

        let results = search_by_name(&idx, "trit", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["_key"], 34);
    }

    #[test]
    fn search_by_name_respects_limit() {
        let fixture = "{\"_key\":1,\"name\":{\"en\":\"Alpha\"}}\n{\"_key\":2,\"name\":{\"en\":\"Alpha Two\"}}\n{\"_key\":3,\"name\":{\"en\":\"Alpha Three\"}}\n";
        let (_f, path) = write_fixture(fixture);
        let pb = indicatif::ProgressBar::hidden();
        let idx = crate::scan::scan_index_pub(&path, &pb).unwrap();

        let results = search_by_name(&idx, "alpha", 2).unwrap();
        assert_eq!(results.len(), 2);
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
    fn apply_language_filter_skips_non_localized_objects() {
        let mut val = serde_json::json!({"typeID": 34, "quantity": 1});
        apply_language_filter(&mut val, "en");
        assert_eq!(val["typeID"], 34);
        assert_eq!(val["quantity"], 1);
    }
}
