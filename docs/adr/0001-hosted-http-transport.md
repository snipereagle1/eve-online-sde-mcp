---
status: accepted
---

# Add a hosted HTTP transport alongside stdio

## Context

The server has always been a stdio-only MCP server: a local MCP client (Claude
Desktop, `.mcp.json`, the MCPB bundle) spawns the binary per session and talks
over stdin/stdout. We want a **hosted** deployment — one long-lived container,
reachable over the network, serving many remote clients — so users can point a
client at a URL instead of building and spawning the binary locally.

This is a transport and deployment-model change, not just packaging. The
decision that follows is hard to reverse (a second transport becomes a public
contract with its own config surface and operational expectations), surprising
without context (a reader will wonder why a stdio MCP server contains an HTTP
server, an axum router, and a `/health` route), and the result of trade-offs
picked at each fork below.

## Decision

Add a network-reachable **Streamable HTTP** transport (rmcp
`transport-streamable-http-server`) that **coexists** with stdio, selected by a
new `--transport stdio|http` flag defaulting to `stdio`. The
download-and-scan half of startup is shared; only the final `serve` step
branches on transport.

Specifics:

- **Stateless sessions.** No per-client session state, no server→client SSE
  push. The tools are pure request/response over an immutable, read-only store,
  so any replica can serve any request — horizontal scaling needs no session
  affinity or shared session store.
- **Language stays deployment-global.** `--language` remains fixed at
  construction; the shared immutable store serves every client. Per-client
  language is deferred and can be added later without redoing transport work.
- **No built-in authentication.** The SDE is public, read-only game data. Access
  control and rate limiting belong to a reverse proxy / ingress, not this
  binary.
- **Startup-only update check.** The SDE build is checked once before serving,
  as today. A long-lived container picks up a new build by restarting, not by
  in-process hot-swap.
- **Config surface.** `--bind` (default `127.0.0.1:8080`), `--path` (default
  `/mcp`), and `--transport`, each with an env-var equivalent
  (`SDE_BIND`, `SDE_PATH`, `SDE_TRANSPORT`) matching the existing
  `SDE_DATA_DIR` / `SDE_LANGUAGE` pattern. The binary binds loopback by default;
  the container image overrides to `0.0.0.0`, making network exposure an
  explicit, visible line in the deployment rather than a silent default.
- **`GET /health`** returns `200` with `{status, build, release_date}` in HTTP
  mode only. Because the port binds only after download + scan complete, a `200`
  honestly means "data loaded, ready" — it backs liveness, readiness, and
  startup probes with one endpoint.
- **Graceful shutdown on `SIGTERM`** for clean `docker stop` / pod termination.

Packaging and deployment:

- **Distroless runtime** (`gcr.io/distroless/cc-debian12:nonroot`, uid 65532)
  via a multi-stage build. TLS is **rustls** (already reqwest 0.13's default — no
  OpenSSL anywhere in the tree, so no migration was needed); CA roots come from
  the OS trust store that distroless/cc ships. No `ca-certificates` package to
  add, no shell in the final image. The arm64 image is produced by
  cross-compiling on the native builder (no QEMU-emulated Rust compile).
- **Data dir pinned to `/data`** (`SDE_DATA_DIR=/data`, `VOLUME /data`, writable
  by uid 65532); the SDE is downloaded at runtime into an optional volume.
- **Distribution:** Dockerfile + `.dockerignore` + `compose.yaml`, plus a
  GitHub Actions workflow publishing a **multi-arch (amd64/arm64)** image to
  GHCR, tagged to match the repo's existing release convention — the **short
  commit SHA** (one release per CI-passing push to `main`) plus `latest`. (There
  are no semver git tags in this repo; releases are per-commit short-SHA.)
- **Kubernetes reference** in `deploy/k8s/`: a Deployment using **`emptyDir`**
  (per-pod runtime download, no PVC or storage-class coupling), a Service, and a
  **`startupProbe`** on `/health` (`failureThreshold: 30`, `periodSeconds: 10`
  ≈ 5-minute budget) that gates liveness/readiness so a cold ~81 MB download
  cannot trigger CrashLoopBackOff. Configured via `env:` only.

## Considered options

- **HTTP replaces stdio.** Rejected: it would break every existing integration
  (`.mcp.json`, MCPB bundle, Claude Desktop) for no benefit to local users. The
  two transports share all startup cost, so coexistence is nearly free.
- **Per-client language.** Deferred: it touches every tool that emits localized
  fields, and it is separable from the transport work. Single-language endpoints
  cover the dominant case.
- **Built-in auth.** Rejected: reimplements what a proxy does better, to protect
  data that is not sensitive.
- **In-process periodic SDE refresh.** Rejected: hot-swapping the store behind a
  lock while serving is a large complexity increase for a data source that
  changes every few weeks, where a container restart is a non-event.
- **Bake the SDE into the image.** Rejected: bloats the image (200 MB+) and
  freezes it to one build, fighting the self-updating design. Runtime download
  keeps the image slim and build-agnostic.
- **debian-slim.** Rejected in favor of distroless: a larger image with a shell
  and more packages, versus a ~60 MB non-root distroless image. (No TLS backend
  trade-off in practice — reqwest was already rustls.)
- **Shared RWX PVC / StatefulSet per-pod PVC for k8s.** Rejected for the default:
  a shared RWX volume makes the advisory download lock unreliable over NFS and
  invites cross-pod extract races; a StatefulSet is heavier than the stateless
  service warrants. `emptyDir` trades a re-download per pod start (cheap, rare)
  for zero storage coupling.
- **`/metrics` (Prometheus).** Explicitly out of scope for v1; an additive change
  if request metrics are wanted later.

## Consequences

- The HTTP transport feature and axum/hyper are compiled into every build,
  including the stdio-only local binary. Accepted as the cost of one binary,
  two transports.
- Stateless is a forward constraint: adding server-initiated notifications or
  subscriptions later would require revisiting the session model.
- Each Kubernetes pod (and each container restart without a persistent volume)
  re-downloads the SDE from CCP. Acceptable given restarts are rare and are the
  intended update mechanism; revisit with a per-pod PVC only if download time
  hurts rollout speed.
- The distroless image has no shell, so `docker exec` debugging is unavailable;
  use a debug build in a slim image when that is needed.
- Memory request/limit for k8s must be measured (post-scan RSS) during
  implementation and documented in the manifest, not guessed.
