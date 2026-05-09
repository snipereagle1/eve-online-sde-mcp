---
name: eve-sde-mcp
description: Rust MCP server exposing EVE Online Static Data Export to AI agents via byte-offset indexed JSONL files
status: active
created: 2026-05-09T22:30:11Z
---

# PRD: eve-sde-mcp

## Executive Summary

`eve-sde-mcp` is a Rust-based Model Context Protocol (MCP) server that exposes the EVE Online Static Data Export (SDE) to AI agents. It manages a locally cached copy of the SDE JSONL files, automatically keeping them current by checking CCP's latest release on every startup. Queries are served by maintaining a lightweight in-memory byte-offset index per file — enabling O(1) record retrieval by ID and fast name search with no full-file deserialization and no intermediate database.

Distributed as an MCPB bundle for one-click installation in Claude Desktop and other compatible MCP clients.

## Problem Statement

EVE Online AI tooling needs access to static game data (types, blueprints, maps, market groups, etc.) without incurring round-trip latency to third-party APIs. Existing solutions require an intermediate database, frequent migrations when the SDE schema changes, or constant network access. Agents need deterministic, fast, offline-capable queries against current SDE data.

## User Stories

- As an AI agent, I can look up any EVE type by ID or name in under 5ms so I can answer player questions without API round-trips.
- As an AI agent, I can find the manufacturing blueprint for any item so I can give accurate industry advice.
- As an AI agent, I can compute the shortest route between two solar systems so I can give navigation guidance.
- As a developer, I can install the server with one click in Claude Desktop so I don't need to configure databases or APIs.
- As a developer, the server always uses the latest CCP build so I don't need to manually update fixtures.

## Functional Requirements

1. Startup update check via HEAD request to CCP's stable redirect URL.
2. Download, extract, and verify (~81 MB zip) when a new build is detected.
3. In-memory byte-offset index (id + name) for all JSONL files, rebuilt on startup.
4. MCP stdio transport with 20+ tools across 8 categories (see §9 of technical spec).
5. BFS route finding over stargate adjacency graph.
6. Product-to-blueprint reverse map built during startup scan.
7. Localization filter via `SDE_LANGUAGE` env var.
8. Progress bars to stderr during download and scan; no stdout except JSON-RPC.

## Non-Functional Requirements

- Startup scan < 10 seconds on full SDE.
- ID lookup < 5ms.
- Name search across types < 50ms.
- Route BFS worst-case < 500ms.
- In-memory index footprint < 100 MB.
- No stdout except valid MCP JSON-RPC frames.

## Success Criteria

- [ ] Startup update check and download complete before tools are available.
- [ ] Startup scan of full SDE < 10 seconds.
- [ ] ID lookup < 5ms.
- [ ] Name search < 50ms.
- [ ] Route BFS < 500ms.
- [ ] Index memory < 100 MB.
- [ ] MCPB bundle installs and runs in Claude Desktop on Linux, Windows, macOS.
- [ ] GitHub Action detects new CCP builds and opens bump PRs.
- [ ] Test fixture suite covers all tool handlers without real SDE download.

## Constraints & Assumptions

- Rust implementation only; no Python/Node alternatives.
- stdio transport only (no REST/HTTP).
- Targets current SDE schema (post-Sept 2025 rework, build 2960198+).
- Open questions resolved: spawn_blocking for BFS, serial scan, global-only language, skip v2 bump validation.

## Out of Scope

- REST/HTTP interface.
- ESI (live game API) integration.
- Pre-rework SDE format support.
- Multi-server or clustered deployment.
- Italian translation fields (removed from SDE in 2025 rework).
- Per-tool language override.

## Dependencies

- `rmcp` crate for MCP stdio transport.
- CCP's SDE distribution at `https://developers.eveonline.com/static-data/`.
- GitHub Actions for CI, release, and SDE bump workflows.
- `mcpb` tool for bundle packaging.
