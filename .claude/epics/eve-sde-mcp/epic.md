---
name: eve-sde-mcp
status: backlog
created: 2026-05-09T22:30:11Z
updated: 2026-05-10T20:32:49Z
progress: 75%
prd: .claude/prds/eve-sde-mcp.md
github: https://github.com/snipereagle1/eve-online-sde-mcp/issues/1
---

# Epic: eve-sde-mcp

## Overview

Rust MCP server exposing EVE SDE to AI agents. Byte-offset indexed JSONL files — no DB, no migrations. Auto-downloads latest CCP build on startup. Distributed as MCPB bundle.

## Architecture Decisions

- stdio transport only (rmcp crate). No REST.
- In-memory id/name index per JSONL file. Rebuild on startup. No index files on disk.
- BFS route finding on `spawn_blocking` thread. Avoids blocking async executor.
- Serial startup scan. Fast enough for v1. Rayon deferred.
- Global `SDE_LANGUAGE` only. No per-tool override.
- Old `sde-{build}/` dirs deleted after successful update.

## Technical Approach

### Core Data Flow
```
startup → HEAD check → download if stale → extract → scan all JSONL → build SdeStore → register tools → stdio loop
```

### SdeStore
One `SdeIndex` per JSONL file: `id_index: HashMap<u64,u64>` + `name_index: HashMap<String,u64>` (byte offsets). Plus `product_to_blueprint` and `stargate_graph` derived maps.

### Query Pattern
Seek file to offset → read one line → parse one JSON object. No full-file deserialize at query time.

### MCP Layer
`rmcp` crate. Tools registered with `Arc<SdeStore>`. Errors surface as MCP tool errors with human-readable message.

## Implementation Strategy

Build in dependency order:
1. Scaffold → storage → download → scan → MCP layer
2. Tool groups in parallel once MCP layer complete
3. Tests + CI/MCPB last

## Task Breakdown Preview

| # | Task | Parallel | Depends |
|---|------|----------|---------|
| 001 | Project scaffold | no | — |
| 002 | Storage & configuration | no | 001 |
| 003 | SDE download & update check | no | 002 |
| 004 | Startup scan & SdeStore | no | 003 |
| 005 | MCP layer & tool framework | no | 004 |
| 006 | Type & item tools + sde_status | yes | 005 |
| 007 | Blueprint tools | yes | 005 |
| 008 | Map tools + BFS route | yes | 005 |
| 009 | Market, dogma, NPC, skin tools | yes | 005 |
| 010 | Tests, GitHub Actions, MCPB bundle | no | 006,007,008,009 |

## Dependencies

- `rmcp`, `serde_json`, `reqwest` (blocking), `zip`, `tokio`, `tracing`, `anyhow`, `clap`, `sha2`, `indicatif`
- CCP SDE distribution endpoint
- `mcpb` CLI for bundle packaging
- GitHub Actions runners with cross-compile targets

## Success Criteria (Technical)

- All 20+ MCP tools respond correctly to fixture data
- Startup scan < 10s on full SDE
- ID lookup < 5ms, name search < 50ms, BFS < 500ms
- Index memory < 100 MB
- No stdout except JSON-RPC frames
- MCPB installs on Linux x86_64, Windows x86_64, macOS ARM

## Estimated Effort

Tasks 001–005 sequential. Tasks 006–009 parallel. Task 010 final.

## Tasks Created

- [ ] 001.md - Project Scaffold (parallel: false)
- [ ] 002.md - Storage & Configuration (parallel: false)
- [ ] 003.md - SDE Download & Update Check (parallel: false)
- [ ] 004.md - Startup Scan & SdeStore (parallel: false)
- [ ] 005.md - MCP Layer & Tool Framework (parallel: false)
- [ ] 006.md - Type & Item Tools + sde_status (parallel: true)
- [ ] 007.md - Blueprint Tools (parallel: true)
- [ ] 008.md - Map Tools + BFS Route (parallel: true)
- [ ] 009.md - Market, Dogma, NPC & Skin Tools (parallel: true)
- [ ] 010.md - Tests, GitHub Actions & MCPB Bundle (parallel: false)

Total tasks: 10
Parallel tasks: 4 (006–009 run concurrently)
Sequential tasks: 6
