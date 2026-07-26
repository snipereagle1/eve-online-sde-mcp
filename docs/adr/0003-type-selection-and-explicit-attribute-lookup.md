# One Type-selector tool, backed by an ExplicitValue inverted index

## Context

The server could resolve a Type to its data in every direction except the one
that matters for discovery: **it could not enumerate a set of Types matching a
predicate.** Every index was a forward lookup (`id → offset`, `name → offset`)
or a reverse map keyed by something other than a Type (`effect_to_types`,
`attribute_modifiers`, `product_to_blueprint`).

The concrete failure: *"which ships have a non-default jump fatigue
multiplier?"* An agent could not search dogma attributes by name (the
`dogmaAttributes` / `dogmaEffects` `name_index` is empty — `extract_name_en`
requires `"name":{"en":…}` and those two files store a bare string), so it found
attribute 1971 only by dumping a Redeemer's full dogma and reading it. It then
called `sde_get_modifiers(attribute_id: 1971)`, which returns **empty** —
nothing in the SDE modifies 1971 — and read that as "no data". It could not list
the Types in a Group or Category to build a candidate set. Having finally
assembled 60 type IDs by hand, batching their dogma cost **200 KB** to read two
fields.

Measured against SDE build 3444265:

| | |
|---|---|
| Types / `typeDogma` rows | 52,821 / 26,827 |
| `(type, attribute, value)` explicit rows | 645,701 across 2,141 attributes |
| rows whose value equals the attribute's default | 102,082 (**15.8%**) |
| attributes where *every* explicit row equals default | 111 of 2,141 |
| Types carrying an explicit attribute 1971 | 66 (values 0.1, 0.25) |
| Types with a `metaGroupID` | 13,789 (**26%**) |
| baseline startup / peak RSS | 0.48 s / 22.8 MB (k8s request 64Mi) |

## Decision

**One selector tool, `sde_find_types`,** rather than the four narrow tools the
gap suggests (`find_types_by_attribute`, `list_types_in_group`,
`list_groups_in_category`, filters on `sde_search_types`). Predicates AND
together: `query`, `type_ids`, `group_ids`, `category_ids`, `meta_group_ids`,
`attribute {id, op?, value?}`, `published_only`, `project_attributes`, `limit`.
Plus `sde_search_dogma` for attribute/effect discovery. Four points are
deliberate and would otherwise read as arbitrary:

**1. The attribute predicate is ExplicitValue-only.** It matches Types with a
stored row in `typeDogma`, never Types sitting at the attribute's
`DefaultValue`. This is not laziness: every Type *has* every DogmaAttribute, so
effective-value semantics would make `{id: 1971, op: "eq", value: 1.0}` return
52,692 Types. Because 15.8% of stored rows coincidentally equal their default,
"has a row" and "has a non-default value" are genuinely different questions —
hence the `not_default` op. The response states its own semantics and the
attribute's default so a caller cannot mistake absence for a value.

**2. A scan-time inverted index, not a table scan.** `attribute_types:
HashMap<u32, Vec<(u32, f32)>>` is built inside the existing `scan_type_dogma`
pass, which already full-parses every line for `effect_to_types`. Cost: ~7.4 MB
and ≤78 ms. A table scan costs 0 MB but **66 ms per query**, warm — and
`src/http.rs` serves concurrent clients, so scans serialise on CPU where a map
hit does not. The index also supplies the `(type, attribute)` random access that
projection needs.

**3. Minimal rows, single-language names.** A row is `{type_id, name, group_id,
attributes?}` ≈ 55–75 B. `--language` is server-global and **off by default**,
so full records carry all eight languages: the 66-Type answer is 5 KB as minimal
rows and **454 KB** as full records. `name` is therefore resolved to one
language (server language, else `en`) even in all-languages mode — a deliberate
divergence from every other tool, because a selector that returns eight-language
name objects costs 5× per row for zero selection value. Callers escalate to
`sde_get_types` / `sde_get_types_dogma` for detail.

**4. Filter → sort → truncate, and say so.** Predicates apply to the whole
candidate set, results sort by `type_id` for run-to-run determinism, then
truncate. `total_matched` / `returned` / `truncated` always present. The `groups`
rollup is computed over the **full** match set, not the returned page, so it
stays correct under truncation — which is what lets it subsume
`list_groups_in_category` honestly. `excluded_no_meta_group` is reported
whenever `meta_group_ids` is active, since 74% of Types have no `metaGroupID`
and absence must not read as Tech I.

### The wire contract

`sde_find_types` is built over three tickets (#40 attribute predicate, #41
taxonomy, #42 MetaGroup) that are three passes over one handler. They are
coupled by the shape of the response, not by the file, so the names are decided
here rather than discovered three times.

Request: `query`, `type_ids`, `group_ids`, `category_ids`, `meta_group_ids`,
`published_only`, `project_attributes`, `limit`, and `attribute: {id, op?,
value?}`. All predicates AND. `limit` defaults to **100**.

`op` is one of `exists` (the default when `op` is omitted; `value` is ignored),
`eq`, `ne`, `gt`, `gte`, `lt`, `lte`, and `not_default`. Every operator is
already restricted to Types holding an ExplicitValue — `exists` is that
restriction alone, and `not_default` further drops rows whose ExplicitValue
equals the DogmaAttribute's DefaultValue. `ne` compares against a caller-supplied
`value`; `not_default` compares against the attribute's own default. They are
different questions and both are needed. The comparison operators require
`value`; omitting it is an error, not a silent fallback to `exists`.

Response: `types` (the rows), `total_matched`, `returned`, `truncated`,
`groups` (the rollup, over the full match set), and — when `meta_group_ids` is
active — `excluded_no_meta_group`. When an `attribute` predicate is present the
response also carries `attribute_semantics`, stating that only ExplicitValues
were considered, and `attribute_default`, echoing the DefaultValue. A row is
`{type_id, name, group_id}`, plus `value` under an attribute predicate and
`attributes` under `project_attributes`.

Calling with no predicate at all is an error naming the available predicates,
never a full dump — naming only the ones that already work, so the message stays
truthful as later passes extend it.

The contract above is the finished shape, not the shape after any one ticket. It
is delivered by #40 (`attribute`, `limit`, and the `total_matched` / `returned` /
`truncated` / `attribute_semantics` / `attribute_default` envelope), #41
(`group_ids`, `category_ids`, `published_only`, and the `groups` rollup), #42
(`meta_group_ids` and `excluded_no_meta_group`), and #48 (`query`, `type_ids`,
`project_attributes`). A request field is added by the ticket that makes it do
something: a field that appears in the JSON schema while silently doing nothing
is worse than an absent one, because a caller passing `group_ids` would read an
unfiltered result as filtered. So a key missing from an earlier pass is that
schedule working, not an omission to fix.

## Considered Options

- **Four narrow tools**, as originally proposed — rejected. The server already
  exposes 28 tools and each addition costs tool-selection accuracy for every
  client; combined predicates ("Black Ops hulls with attr 1971") would still
  need client-side joining.
- **Table scan on demand** — rejected on the HTTP transport, not on latency.
  66 ms is tolerable for one stdio client and not for N concurrent HTTP ones.
- **Effective-value attribute semantics** — rejected: correct to EVE's engine,
  useless as a filter, and incompatible with a sparse index.
- **Full type records in results** — rejected at 454 KB for the motivating
  query.
- **A separate `sde_list_groups(category_id)`** — rejected in favour of the
  rollup, which answers the taxonomy question as a by-product of a real query.
  Deriving the taxonomy from the returned rows instead would be *wrong*: under
  `truncated: true` you would be reading the group distribution of a capped
  page (100 of category 91's 11,836 Types).
- **Folding the attribute predicate into `sde_get_modifiers`** as a fourth
  direction — rejected. It reuses that tool's exactly-one-of dispatch, but the
  return shape is Types-with-values rather than modifiers, and the
  group/category/meta filters and rollup have nowhere natural to sit.

## Consequences

- Startup 0.48 s → ~0.6 s; peak RSS 22.8 MB → ~32 MB, against a 64Mi k8s
  request and 128Mi limit. The added indexes are `attribute_types` (~7.4 MB),
  `groupID` + `metaGroupID` extracted in the existing `types.jsonl` memmem pass
  (~0.9 MB, +40 ms), and a dogma text corpus for `sde_search_dogma` (<1 MB).

  Measured after #41 (release builds, `/usr/bin/time -v`, three runs, against a
  baseline worktree with #40 but not #41): peak RSS 29.0 → 30.2 MiB, **+1.1 MiB**,
  with no measurable change in wall-clock startup. The `types.jsonl` pass costs
  78 ms against 55 ms for a plain `scan_index` — **+23 ms** against the budgeted
  +40. No drift signal. The worst rollup in the build is category 11 at 391
  Groups: 5.9 ms and 32 KB, of which ~25 KB is rollup; #42 adds to that same
  envelope.

- The store gained two fields beyond the four enumerated above, both for the
  same reason the taxonomy indexes exist — a predicate that applies to the
  **full** match set must not cost a seek and parse per candidate:

  `category_groups` (categoryID → its Groups, 47 entries). "Resolve through
  `groups.jsonl`" works for a known Group, because `id_index` is `groupID →
  offset`; a Category filter runs the other way and there is no `categoryID`
  index, so answering it from the existing index means seek + parse of all 1,609
  Group records. Measured: **26 ms per query versus 882 ns** for the map. That
  is this ADR's own rejected-option argument one level down — the table scan was
  rejected not on latency but because `src/http.rs` serves concurrent clients
  and scans serialise on CPU where a map hit does not.

  `published_types` (the 26,983 published Types, ~0.25 MiB). Membership rather
  than a flag folded into `type_group`, which would have been free in that
  value's padding: `type_group` should stay Type→Group so that #42's sparse
  `type_meta_group` sits beside it rather than widening a tuple, and a set is
  read as what its name says. Stored as the published side, not the smaller one
  — the split is 51/49 so there is no smaller side, and storing the unpublished
  side would make a Type absent from `types.jsonl` read as *published*, which is
  the wrong direction for a filter whose job is excluding junk.
- `manufacturing.rs::meta_group_of` stops doing a seek + read + full JSON parse
  per Type and becomes an O(1) map hit — a side win on deep production chains.
- `query.rs::search_by_name` changes from *iterate `HashMap` → take(limit)* to
  *collect → sort → filter → take(limit)*. This fixes two live defects in
  `sde_search_types`: nondeterministic result ordering across runs, and
  `published_only` filtering *after* the limit, so `limit:10,
  published_only:true` could return 3 while thousands matched.
- `sde_find_types` and `sde_get_modifiers` answer near-identical English
  questions ("which Types **have** attribute X" vs "which Types **modify** it")
  with disjoint data. Beyond describing the contrast, each tool points at the
  other on an empty result — this is the specific rescue for the failure that
  motivated the ADR, where an empty `sde_get_modifiers` response was read as
  "no such data" while 66 Types carried the attribute.
- `sde_search_dogma` must match `displayName` and `description`, not just
  `name`. Attribute names are camelCase identifiers, so `"fatigue"` finds 1971
  but `"jump fatigue"` — the phrasing an agent reaches for first — finds nothing
  by name and only resolves via `"Jump Fatigue Multiplier"` / `"Multiplier for
  jump fatigue distance"`.
- A `sde_find_types` call costs one O(1) seek for the queried attribute's
  `DefaultValue` plus one per returned row. Caching the 2,141 defaults in memory
  would remove exactly one seek of a default-limit query's 101, so it is not
  worth a fourth custom scanner over `dogmaAttributes.jsonl` on its own — but
  `sde_search_dogma` has to hold every attribute's name, `displayName` and
  `description` in memory anyway, at which point `defaultValue` rides along for
  nearly nothing. It belongs with that corpus, not before it. Revisit if
  multiple simultaneous attribute predicates land, since the seek count then
  scales with predicate count.
- Deferred, not rejected: multiple simultaneous attribute predicates, offset
  pagination, and `effect_ids` projection.
