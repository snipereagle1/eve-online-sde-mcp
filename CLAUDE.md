# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Skills

- `sde` — how to read the EVE Static Data Export: JSON Lines layout, per-file schemas, ID/name resolution, dogma, blueprints, the map. Consult before touching scanning or query code for an SDE file you haven't worked with.
- `sde-vs-esi` — decide whether a piece of data belongs to the SDE or to ESI. Read this first whenever a request could plausibly want live/character data; this server only serves static data.
- `esi` — ESI API discipline (auth, caching, error limits). Only relevant for judging what stays out of scope here.
- `domain-modeling` — maintaining `CONTEXT.md` and `docs/adr/`; use when introducing or renaming domain terms, or recording an architectural decision.
- `tdd` — for new tools and bug fixes; tests use `tempfile` JSONL fixtures (see "Important constraints").
- `diagnosing-bugs` — for reported breakage, scan/parse failures, or startup performance regressions.

Delivery work is tracked in GitHub Issues, not in a planning skill — see "Agent skills" below.

## Commands

```bash
cargo build                          # build
cargo test                           # run all tests
cargo test <name>                    # run single test by name substring
cargo test -- --ignored              # run network integration tests
cargo run -- --help                  # CLI flags
cargo run -- --redownload            # force re-download SDE
RUST_LOG=debug cargo run             # run with debug logging
```

## Architecture

**What it is**: A Rust MCP server (stdio transport via `rmcp`) exposing EVE Online SDE data to AI agents. No database — uses byte-offset indexed JSONL files for O(1) ID lookups with no full-file deserialization.

**Startup flow** (`main.rs`):
1. `download::check_and_update` — HEAD checks CCP's stable redirect URL, downloads+extracts the ~81 MB zip if build number changed, stores `meta.json` with current build
2. `scan::scan_sde` — reads all 17 JSONL files, builds in-memory `HashMap<id, byte_offset>` and a `NameIndex` (`name_lowercase` -> the `_key`s carrying it) per file; also builds `product_to_blueprint` reverse map, `stargate_graph` adjacency map, and `attribute_modifiers` (reverse map from `dogmaEffects.modifierInfo`, keyed by `modifiedAttributeID`)
3. `SdeMcpServer::serve` — runs MCP stdio transport with 32 tools

**Data access pattern** (`tools/query.rs`):
- ID lookup: `id_index.get(id)` → seek to byte offset → read one line → deserialize
- Name search: iterate `name_index`, check `contains(query)`, collect the matching IDs, filter them, sort ascending, truncate to the limit, then seek+read — filtering and ordering happen before the cap, never after
- Language filter: `apply_language_filter` recursively replaces `{"en": ..., "de": ...}` objects with the chosen language string (falls back to `"en"`)

**Key files**:
- `src/store.rs` — `SdeStore` (all indexes) and `SdeIndex` (path + id_index + name_index)
- `src/scan.rs` — JSONL scanning; `scan_blueprints`, `scan_stargates`, and `scan_dogma_effects` have custom parsers for their derived structures
- `src/tools/server.rs` — all 32 MCP tool definitions using `#[tool]` / `#[tool_router]` macros; `fetch_filtered` and `search_filtered` helpers apply language filter. `sde_get_skill_plan` (recursive prereq traversal + topo sort + SP math) and `sde_get_modifiers` (dogma modifier resolution) live here as free functions below the impl. `sde_find_types` is the Type selector (predicates AND; `attribute` / `group_ids` / `category_ids` / `type_ids` produce a candidate set, `meta_group_ids` / `published_only` / `query` only narrow one), and `sde_search_dogma` finds DogmaAttributes and DogmaEffects by name
- `src/download.rs` — SDE download; extracts build number from CCP redirect URL
- `src/config.rs` — CLI args (clap) and `Meta` (persisted build state)

**SDE data directory layout**: `~/.local/share/eve-sde-mcp/sde-{build}/` containing the extracted JSONL files. Old build dirs are deleted on successful download.

**Adding a new tool**: add a field to `SdeStore` + `SdeIndex` in `store.rs`, scan it in `scan.rs`, add a `#[tool]` method to `SdeMcpServer` in `tools/server.rs`.

## Configuration

| Flag / Env | Default | Purpose |
|---|---|---|
| `--data-dir` / `SDE_DATA_DIR` | `~/.local/share/eve-sde-mcp` | SDE cache directory |
| `--language` / `SDE_LANGUAGE` | (all langs returned) | Filter localized name fields |
| `--log-level` | `warn` | Tracing level; use `RUST_LOG` to override |
| `--redownload` | false | Force re-download even if build is current |

## Important constraints

- **No stdout except MCP JSON-RPC frames** — all progress bars, logs, and status messages go to stderr. Breaking this breaks MCP clients.
- `scan_index` uses `memchr::memmem` for fast byte-pattern matching to extract `_key` and `name.en` without full JSON parsing — the hot path for startup.
- Tests use `tempfile` JSONL fixtures; the `scan_index_pub` re-export in `scan.rs` exists solely to expose the private function to tests in `tools/server.rs`.

## Agent conventions

### Issue tracker

Issues live in GitHub Issues (`snipereagle1/eve-online-sde-mcp`). See `docs/agents/issue-tracker.md`.

### Triage labels

Default label vocabulary (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context — `CONTEXT.md` + `docs/adr/` at the repo root. See `docs/agents/domain.md`.
