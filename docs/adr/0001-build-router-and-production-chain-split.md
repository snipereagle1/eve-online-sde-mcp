# Split manufacturing planning into a classify-only router and a compute engine

## Context

Agents asked to "build a Type" (e.g. a Nightmare hull) thrash badly with the
current tools: they discover the production tree one `sde_get_blueprint_for_product`
call at a time, misread an ambiguous `null` as "raw" when it is really a
reaction output, and negotiate the buy-vs-build boundary across many turns of
correction. The data to do better is all present in the SDE; the tooling just
does not expose it.

## Decision

Add two tools with a hard division of labor:

- **`sde_build_type` (router)** — walks the *full* production tree but does
  **classification only, no quantity math**. It returns the target's buildability,
  the set of decomposable Origins present in the tree, aggregate required skills,
  flagged out-of-scope leaves, and the buy-vs-build decision gates. It stays
  **neutral** (facts, not recommendations) and **self-corrects toward
  manufacturing** so a speculative or mis-routed call still steers the
  conversation onto the build path.
- **`sde_get_production_chain` (engine)** — given the human's decisions, does the
  **full quantity math**: per-job ME, whole-batch run rounding, leftovers,
  tree-wide demand consolidation, and the consolidated raw shopping list.

The two run as **separate sequential calls** with a human decision in between,
not one tool with a `mode` flag.

## Considered Options

- **One tool with a `discover | compute` mode** — rejected. It muddies two very
  different contracts (one is read-only and cheap, one computes and depends on
  decisions), and there is no natural place for the human-in-the-loop pause.
- **A one-level router** — rejected. The buy-vs-build choice (e.g. "build
  reactions, or buy the outputs?") requires knowing reactions exist 2–3 tiers
  down; a one-level router cannot surface the decision that matters, so the user
  would again *discover* it through correction.

## Consequences

- The router must do a full-depth tree walk even though it computes no
  quantities — cheap (O(1) blueprint lookups, ~4 tiers, ~dozen nodes).
- Origin classification lives in one place (the router's rules) and the engine
  trusts it rather than re-deriving — the two tools must share the classifier.
- Both depend on a prerequisite data-layer fix: the product→blueprint reverse
  map must index reaction activities (not manufacturing only) and tag each entry
  with its activity, so `null` means "genuinely no formula" rather than
  "reaction not indexed."
