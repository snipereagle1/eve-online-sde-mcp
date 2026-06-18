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
