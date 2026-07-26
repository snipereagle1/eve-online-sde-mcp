# The SDE test fixture

A hand-picked slice of the real SDE (build 3444265), used by `mod mcp_seam` in
`src/tools/server.rs` to boot a real scan and a real MCP server.

**Records are verbatim excerpts, not inventions.** Fields are subsetted — names,
IDs, dogma values and language maps are copied from the real files. Two
deliberate exceptions are called out below. If you add to the fixture, copy from
the real SDE rather than writing plausible-looking JSON; a fabricated shape here
turns into a test that passes while production returns nothing.

## Schema notes that are easy to get wrong

- `dogmaAttributes` and `dogmaEffects` store `name` as a **bare string**, not a
  `{"en": …}` map. Every other file uses the map. This is why the `name_index`
  for those two files is empty — `extract_name_en` requires `"name":{`.
- `dogmaAttributes.description` is a **bare string** (English only), but
  `dogmaEffects.description` is a **localized map**. Anything searching dogma
  text has to handle both.
- Localized maps carry all eight SDE languages (`de en es fr ja ko ru zh`, per
  `translationLanguages.jsonl`). Keep it that way: an en-only fixture hid a live
  bug where `apply_language_filter` rejected every real record because `es` was
  missing from `LANG_CODES`.
- Attributes 277/278 have **no** `displayName` in the real SDE. That absence is
  intentional — dogma search must not assume the field exists.

## What each behaviour is pinned by

| Behaviour | Fixture data |
|---|---|
| Attribute predicate, non-default | attr 64 `damageMultiplier` (default 1.0): Hobgoblin II (2456) stores 1.92 |
| Attribute predicate, **equal to default** | attr 64 on the four mining drones (1202, 3218, 10248, 10252), all storing 1.0 — this is what stops a `not_default` filter passing vacuously |
| Dogma search by spaced phrase | attr 1971 `jumpFatigueMultiplier` → "Jump Fatigue Multiplier" / "Multiplier for jump fatigue distance"; attr 9 `hp` → "Structure Hitpoints". Neither phrase occurs in the camelCase `name` |
| MetaGroup filter | 11 of 27 types carry `metaGroupID` (1, 2 and 4 are all represented) |
| MetaGroup **absence** | Hoarder (651) carries attr 1971 but no `metaGroupID`, so it must not be silently counted as Tech I |
| Group / Category rollup | category 6 Ship spans groups 27, 28, 419, 463, 898; category 16 Skill spans 257 and 1218; category 18 Drone spans 100 and 101 |
| Truncation vs. complete result | group 18 Mineral holds 8 types |
| `published_only` applied before `limit` | 10248 and 10252 are unpublished and both match "mining", alongside five published matches |

Every `groupID` a type references is declared, every group resolves to a
declared category, and every `attributeID` in `typeDogma.jsonl` is declared in
`dogmaAttributes.jsonl`.

## The two deliberate divergences

1. **`typeDogma` rows for 16227, 34, 3386, 3410, 17940, 17476 and 87562 are
   hand-tuned, not real.** They encode the skill-plan scenario: 87562 demands
   Mining **V** where the real SDE says IV, which is the whole point of the
   "deduped to the higher level" assertion. Do not "correct" these against the
   real SDE without rewriting the SP expectations that depend on them.

2. **No type sits at attribute 1971's default.** All 66 real carriers store 0.1
   or 0.25, and the fixture keeps that. A `not_default` test written against
   1971 would therefore pass vacuously — use attribute 64 for that, where the
   fixture has real rows on both sides.

Rows for types whose `requiredSkill1` pointed at a type outside the fixture had
both skill slots dropped, so no prerequisite dangles.
