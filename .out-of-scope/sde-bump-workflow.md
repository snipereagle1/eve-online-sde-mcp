# Automated SDE Bump Workflow

This project does not ship a scheduled GitHub Actions workflow that detects new
CCP SDE builds and auto-opens a PR to bump the pinned build.

## Why this is out of scope

The server already resolves the SDE build **at runtime**. On startup
`download::check_and_update` issues a HEAD against CCP's stable redirect URL,
parses the build number out of the final redirect (`parse_build`), and
downloads + extracts the new zip whenever the build changes — persisting it to
`meta.json`. End users always run against the latest SDE without any repo change.

The only thing a bump workflow would touch is `PINNED_BUILD` in
`src/sde_version.rs`, which exists solely so the committed test fixtures under
`tests/fixtures/sde/` and the offline test suite have a stable reference build.
That constant does not affect what data users get — it only governs CI fixtures.

Automating that bump means standing up a self-PRing workflow with
`contents: write` + `pull-requests: write`, on a weekly cron, to change one
integer that has no runtime effect. The cost (a privileged auto-committing
workflow to maintain) outweighs the benefit. When CCP changes the SDE schema in
a way that matters, the right signal is a **human** noticing test failures or a
parser break — not an automated PR that silently advances the pinned build and
risks masking a schema drift behind a green checkmark.

If the fixtures ever need refreshing, that is a deliberate, human-reviewed
action, not a scheduled job.

## Prior requests

- #17 — "SDE Bump Workflow" (closed wontfix 2026-06-19)
