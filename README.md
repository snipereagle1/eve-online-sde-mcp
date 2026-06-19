# eve-online-sde-mcp

An [MCP](https://modelcontextprotocol.io) server that exposes EVE Online's **Static Data Export (SDE)** to AI agents over stdio. It gives a model fast, structured access to EVE's game data — items, ships, blueprints, dogma attributes, market groups, factions, and the full solar-system map — without a database.

## Highlights

- **No database.** The SDE's JSONL files are scanned once at startup into in-memory byte-offset indexes. Lookups by ID or name seek directly to the relevant line — O(1), with no full-file deserialization.
- **Self-updating data.** On launch the server HEAD-checks CCP's stable redirect URL, and downloads + extracts the ~81 MB SDE zip only when the build number has changed. The current build is cached in `meta.json`.
- **28 query tools** covering items, industry, dogma, navigation, and the market/political hierarchy.
- **Localization-aware.** Localized name fields can be filtered to a single language (defaults to English), falling back to English when a translation is missing.

## Quick start

```bash
cargo build --release
```

Add it to your MCP client. For Claude Code, a `.mcp.json` entry:

```json
{
  "mcpServers": {
    "eve-sde": {
      "command": "/path/to/eve-online-sde-mcp/target/release/eve-sde-mcp",
      "args": []
    }
  }
}
```

To filter localized names to one language, pass `--language` in `args` (or set `SDE_LANGUAGE`):

```json
{
  "mcpServers": {
    "eve-sde": {
      "command": "/path/to/eve-online-sde-mcp/target/release/eve-sde-mcp",
      "args": ["--language", "de"]
    }
  }
}
```

On first run the server downloads and extracts the SDE (~81 MB zip). This takes a moment; subsequent runs reuse the cached build and start in milliseconds.

## CLI flags

| Flag / Env | Default | Purpose |
|---|---|---|
| `--data-dir` / `SDE_DATA_DIR` | `~/.local/share/eve-sde-mcp` | SDE cache directory |
| `--language` / `SDE_LANGUAGE` | `en` | Language for localized name fields |
| `--log-level` | `warn` | Tracing level (override with `RUST_LOG`) |
| `--redownload` | `false` | Force re-download even if the build is current |

```bash
cargo run -- --help          # show all flags
cargo run -- --redownload    # force a fresh SDE download
RUST_LOG=debug cargo run     # run with debug logging
```

> The data directory defaults to `%APPDATA%` on Windows and honors `XDG_DATA_HOME` on Linux.

## Tools

### Status

- `sde_status` — build number, release date, data directory, files scanned.

### Items & industry

- `sde_get_type` / `sde_get_types` — fetch a type (item) by ID, singly or batched.
- `sde_search_types` — substring search by name; optional published-only filter.
- `sde_resolve_types` — bulk id↔name mapping (exact, case-insensitive).
- `sde_get_group` / `sde_get_category` — type classification hierarchy.
- `sde_get_type_materials` — reprocessing material composition of a type.
- `sde_get_blueprint` — a blueprint by its blueprint type ID.
- `sde_get_blueprint_for_product` — reverse lookup: blueprint that makes a product.

### Dogma, skills & modifiers

- `sde_get_type_dogma` / `sde_get_types_dogma` — dogma attributes and effects for a type; `resolve_names` annotates attributes and decodes skill prerequisites into readable form.
- `sde_get_dogma_attribute` / `sde_get_dogma_effect` — look up an attribute or effect definition by ID.
- `sde_get_skill_plan` — recursive skill-prerequisite training plan for one or more target type IDs. Returns each target's prerequisite tree plus one merged, deduped, topologically-sorted plan with per-skill rank, SP cost, running cumulative SP, the per-level SP curve, and which targets need it.
- `sde_get_skill_sp` — SP cost curve (levels 1–5) for a skill, from a rank or a type ID.
- `sde_get_modifiers` — resolve dogma modifiers without prose parsing. Provide exactly one of `type_id` (attributes a thing modifies), `attribute_id` (what modifies an attribute), or `effect_id` (raw `modifierInfo` entries).

### Space & navigation

- `sde_get_solar_system` / `sde_search_solar_systems` — by ID, name, or substring.
- `sde_get_constellation` / `sde_get_region` — navigation hierarchy above systems.
- `sde_find_route` — shortest route between two solar systems (BFS over the stargate graph); returns jump count and the system-ID path.
- `sde_get_npc_station` — an NPC station by ID.

### Economy & politics

- `sde_get_market_group` / `sde_get_market_group_tree` — a market group, or its full root-to-node ancestor chain.
- `sde_get_faction` — a faction by ID.
- `sde_get_npc_corporation` — an NPC corporation by ID.

### Cosmetics

- `sde_get_skin` — a ship SKIN by ID.

## How it works

On startup (`main.rs`):

1. **`download::check_and_update`** — HEAD-checks CCP's stable redirect URL, extracts the build number from the resolved URL, and downloads + extracts the SDE zip only if the build changed. Old build directories are removed after a successful download.
2. **`scan::scan_sde`** — reads all 17 JSONL files and builds, per file, an in-memory `HashMap<id, byte_offset>` and `HashMap<name_lowercase, byte_offset>`. It also derives a product→blueprint reverse map, a stargate adjacency graph, and an attribute-modifier reverse map (from `dogmaEffects.modifierInfo`, keyed by modified attribute ID). The hot path uses `memchr` to extract keys and names without full JSON parsing.
3. **`SdeMcpServer::serve`** — runs the MCP stdio transport exposing the tools.

Data lives under `~/.local/share/eve-sde-mcp/sde-{build}/`.

> **MCP constraint:** stdout carries only MCP JSON-RPC frames. All logs, progress bars, and status messages go to stderr — breaking this breaks MCP clients.

See [`CONTEXT.md`](CONTEXT.md) for the domain model and [`CLAUDE.md`](CLAUDE.md) for architecture and contributor notes.

## Development

```bash
cargo build
cargo test                   # all tests
cargo test <name>            # single test by name substring
cargo test -- --ignored      # network integration tests
```

The MCP protocol harness in `tests/` exercises all tool handlers; unit tests use `tempfile` JSONL fixtures.

### Pre-commit hook

A git hook in `.githooks/` keeps commits clean against the CI gates. Enable it once per clone:

```bash
git config core.hooksPath .githooks
```

On each commit it runs `cargo fmt --all` and re-stages the formatted Rust files automatically (formatting never blocks the commit), then runs `cargo clippy --all-targets -- -D warnings` — clippy warnings are the only thing that aborts the commit. Bypass for a single commit with `git commit --no-verify`.

## Data attribution

The Static Data Export is published by **CCP Games**. EVE Online and all related material are trademarks of CCP hf. This project is an unofficial tool and is not affiliated with or endorsed by CCP.
</content>
