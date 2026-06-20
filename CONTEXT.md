# EVE Online SDE

Domain model for the EVE Online Static Data Export — a snapshot of game data distributed by CCP Games. This server exposes it via MCP tools for AI agents.

## Language

### Items & Industry

**Type**:
The base unit of EVE's item system. Every ship, module, resource, and manufactured product is a Type, identified by a numeric type ID.
_Avoid_: item, object, entity

**Group**:
A first-level classification that Types belong to (e.g. "Frigate" groups all frigate Types). One Type belongs to exactly one Group.
_Avoid_: subcategory, class

**Category**:
The parent classification above Groups (e.g. "Ship" contains Frigate, Cruiser, Battleship groups). The broadest domain organizer.
_Avoid_: domain, type-class

**Blueprint**:
A Type that defines one or more manufacturing or reaction activities. A Blueprint produces a specific output Type; the reverse lookup from output Type to Blueprint is the product-to-blueprint map.
_Avoid_: recipe, schematic

**TypeMaterial**:
The raw material composition of a Type — which materials and quantities are yielded when that Type is reprocessed.
_Avoid_: material list, ingredients

**Origin**:
*How* a Type is produced — an intrinsic property derived from SDE data alone. Fixed for a given Type regardless of any build. Exactly one of six values, resolved blueprint-first (a Type's Blueprint activity is authoritative over its Group): `reaction-output`, `manufactured`, `pi-output`, `mineral`, `moon-material`, `raw-other` (terminal catch-all). Group/Category are a fallback only for Types with no Blueprint. Distinct from Disposition.
_Avoid_: source, kind, type (unqualified)

**Disposition**:
*Whether the player builds or buys* a Type on a specific build — `build` vs `buy`. Contextual: determined by the player's chosen Raw Boundary, not by the Type itself. The same Type (e.g. Reinforced Carbon Fiber) can be `build` on one plan and `buy` on another. Only the production-chain computation knows Disposition; the build router reports Origin only.
_Avoid_: source, build-vs-buy (informal)

**MeMode**:
The ME-research eligibility of a `manufactured` Type, derived from its product metaGroupID (NOT from blueprint activity presence — faction Blueprints carry research activities yet are unresearchable). One of: `researchable` (Tech I — player chooses ME 0–10), `fixed-zero` (Faction/Storyline/Officer/Deadspace — BPC-only, ME locked 0), `invented` (Tech II/III — ME set at invention; out of scope, treated as a leaf/buy and flagged). Absent for non-manufactured Origins (reactions ignore ME; raw/PI have none).
_Avoid_: researchable (as a bool), ME flag

**RawBoundary**:
The set of decomposable Origins a player chooses to *buy* rather than build on a given plan. Only `manufactured` and `reaction-output` are decomposable; `pi-output`, `mineral`, `moon-material`, `raw-other` are always terminal leaves (PI-build and invention are out of scope). The boundary is a player decision surfaced as buy-vs-build options, never silently defaulted. A per-Type override list can force individual Types to `buy` regardless of their Origin (e.g. a skill-gated component). Disposition is the per-Type result of applying the Raw Boundary plus overrides.
_Avoid_: stop-set, raw materials (informal), cutoff

**ProductionChain**:
The full bottom-up decomposition of a target Type into the quantities a player must acquire — recursing through every Type above the Raw Boundary, summing inputs across the tree with per-Blueprint ME and whole-batch run rounding, and emitting consolidated leaf totals plus a run plan. Computed by the chain tool; distinct from the build router, which only classifies one level and surfaces decisions.
_Avoid_: build tree, recipe tree, BOM

**DogmaAttribute**:
A named numeric or categorical property on a Type (e.g. shield capacity, CPU usage). Defines the mechanical stats of ships and modules.
_Avoid_: stat, property, attribute (unqualified)

**DogmaEffect**:
A gameplay rule or behavior that activates on a Type (e.g. "turret hardpoint"). Complements DogmaAttributes to fully specify mechanics.
_Avoid_: ability, modifier

**Skin**:
A cosmetic variant applied to a Type. Does not affect mechanics.
_Avoid_: paint, variant

### Space & Navigation

**SolarSystem**:
A discrete location in EVE space. Has a name, security status, and a parent Constellation. Connects to other SolarSystems via Stargates.
_Avoid_: system (unqualified), star system

**Constellation**:
A grouping of SolarSystems within a Region. Identified by constellation ID.
_Avoid_: sector, zone

**Region**:
The largest geographic unit. Contains multiple Constellations. Identified by region ID.
_Avoid_: area, territory

**Stargate**:
A traversable connection between two SolarSystems. The stargate graph maps each SolarSystem to the set of SolarSystems reachable via its stargates.
_Avoid_: jump, wormhole, gate (unqualified)

**Route**:
A path between two SolarSystems calculated via BFS through the stargate graph. Expressed as an ordered list of SolarSystem IDs and a jump count.
_Avoid_: path, directions

### Economy & Politics

**MarketGroup**:
A node in the market taxonomy used to browse items for trade. Forms a tree via parent group IDs. Independent of Type Groups.
_Avoid_: market category, trade group

**Faction**:
A major political entity in EVE (e.g. Caldari State, Minmatar Republic). Identified by faction ID.
_Avoid_: empire, race, government

**NpcCorporation**:
An NPC-controlled organization. Owns NpcStations and runs missions. Identified by corporation ID.
_Avoid_: corp (unqualified), NPC corp

**NpcStation**:
A player-accessible station in space. Belongs to a SolarSystem and is managed by an NpcCorporation.
_Avoid_: station (unqualified), outpost

## Relationships

- A **Type** belongs to one **Group**; a **Group** belongs to one **Category**
- A **Blueprint** produces a **Type**; the product-to-blueprint map is the reverse lookup
- A **TypeMaterial** describes what reprocessing a **Type** yields
- A **SolarSystem** belongs to one **Constellation**; a **Constellation** belongs to one **Region**
- A **Stargate** links two **SolarSystems**; a **Route** is computed across the stargate graph
- An **NpcStation** belongs to one **SolarSystem** and one **NpcCorporation**
- A **MarketGroup** may have a parent **MarketGroup** (tree hierarchy)
- **DogmaAttributes** and **DogmaEffects** are applied to **Types**

## Example dialogue

> **Dev:** "If I have a blueprint, how do I find the Type it produces?"
> **Domain expert:** "The Blueprint's activities list the output type ID. Use the product-to-blueprint map in reverse — or look up the Blueprint directly by that type ID."

> **Dev:** "Is MarketGroup related to Group?"
> **Domain expert:** "No. Group is a mechanical classification (what a Type *is*). MarketGroup is a market taxonomy (where you find it in the trade interface). They're completely independent hierarchies."

> **Dev:** "What's a Route?"
> **Domain expert:** "A jump path between two SolarSystems through the stargate graph — expressed as the sequence of system IDs and the total jump count."

## Flagged ambiguities

- **"type"** is overloaded: in Rust source it appears as a keyword, a generic, and as the EVE domain concept **Type**. In this codebase, the EVE concept is always `SdeType` or stored under the `types` index — use **Type** when talking about the EVE item archetype.
- **"system"** appears in EVE lore and in general Rust/OS usage. Always qualify: **SolarSystem** for the EVE concept.
- **"station"** appears in EVE broadly (player-built citadels, NPC stations). In the SDE, only NPC stations are present — use **NpcStation** to be precise.
