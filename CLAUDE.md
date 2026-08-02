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
- `src/tools/` — the 32 MCP tools, split one directory per domain, each `mod.rs` + `tests.rs` with its own `#[tool_router]`:
  - `types/` — Type, Group, Category, reprocessing materials, SKINs, and `sde_find_types`, the Type selector (predicates AND; `attribute` / `group_ids` / `category_ids` / `type_ids` produce a candidate set, `meta_group_ids` / `published_only` / `query` only narrow one)
  - `dogma/` — DogmaAttributes and DogmaEffects, `sde_search_dogma`, and `sde_get_modifiers` (modifier resolution). Owns the ExplicitValue-vs-DefaultValue rule; `types/` borrows its projection helpers
  - `skills/` — `sde_get_skill_plan` (recursive prereq traversal + topo sort + SP math) and `sde_get_skill_sp`
  - `manufacturing/` — the `build_type` router and `production_chain` engine, plus their two tools
  - `map/`, `market/`, `politics/`, `blueprints/` — solar systems and routing, market groups, factions and NPC corps/stations, blueprint lookup
  - `query.rs` — byte-offset fetch, name search, language filter, and the corpus substring search (`matching_records`)
  - `guidance.rs` — the empty/truncated-result guidance strings; `testkit.rs` — shared test fixtures and the MCP `Seam` harness
- `src/tools/server.rs` — `SdeMcpServer` itself, the `fetch_filtered` / `search_filtered` language-filter helpers every domain calls, `sde_status`, and the `tool_router()` that sums the per-domain routers
- `src/download.rs` — SDE download; extracts build number from CCP redirect URL
- `src/config.rs` — CLI args (clap) and `Meta` (persisted build state)

**SDE data directory layout**: `~/.local/share/eve-sde-mcp/sde-{build}/` containing the extracted JSONL files. Old build dirs are deleted on successful download.

**Adding a new tool**: add a field to `SdeStore` + `SdeIndex` in `store.rs`, scan it in `scan.rs`, then add a `#[tool]` method to the `#[tool_router]` impl of the domain module it belongs to. A new domain needs its own directory, a `mod` line in `tools/mod.rs`, and a `+ Self::<domain>_router()` in `server.rs`. The pinned `tools/list` contract test enumerates the router automatically, so there is no list to edit — instead regenerate its golden with `SDE_UPDATE_TOOLS_LIST=1 cargo test tools_list_matches_the_pinned_contract`, then read the diff to `tests/fixtures/tools-list.json` and commit it. That run deliberately fails; a green suite means the contract matched without a rewrite.

## Configuration

| Flag / Env | Default | Purpose |
|---|---|---|
| `--data-dir` / `SDE_DATA_DIR` | `~/.local/share/eve-sde-mcp` | SDE cache directory |
| `--language` / `SDE_LANGUAGE` | `en` | Language localized name fields are filtered down to; an empty or unknown code falls back to `en` |
| `--log-level` | `warn` | Tracing level; use `RUST_LOG` to override |
| `--redownload` | false | Force re-download even if build is current |

## Important constraints

- **No stdout except MCP JSON-RPC frames** — all progress bars, logs, and status messages go to stderr. Breaking this breaks MCP clients.
- `scan_index` uses `memchr::memmem` for fast byte-pattern matching to extract `_key` and `name.en` without full JSON parsing — the hot path for startup.
- Tests use `tempfile` JSONL fixtures; the `scan_index_pub` re-export in `scan.rs` exists solely to expose the private function to the tool tests. Shared fixture builders and the MCP `Seam` harness live in `tools/testkit.rs`.

## Agent conventions

### Issue tracker

Issues live in GitHub Issues (`snipereagle1/eve-online-sde-mcp`). See `docs/agents/issue-tracker.md`.

### Triage labels

Default label vocabulary (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context — `CONTEXT.md` + `docs/adr/` at the repo root. See `docs/agents/domain.md`.
