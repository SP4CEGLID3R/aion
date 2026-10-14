# Stage 5 — Supported game versions and missing data

## Status, plainly

| Question | Answer |
|---|---|
| Does the radar read AION 2 data today? | **It captures and frames it**, but the cube and player-position packets are not identified, so nothing is drawn from the game until you find them with the discovery tools and write a validated decoder profile ([guide](./04-packet-capture.md)) |
| Which AION 2 client versions are supported? | **None are verified.** The radar is game-agnostic: it shows whatever a validated source sends in the [Radar Event Protocol](./radar-event-protocol.md) |
| What does it run on today? | Passive packet capture with a decoder profile, the built-in synthetic simulator, replay files (`.jsonl`, `.srf`, `.pcap`, `.pcapng`), and a loopback telemetry socket |
| Can the Aether DPS meter supply the data? | **No.** It parses combat, names, summons and HP only, with no positions and no cubes, and it has no external API ([research](./01-dps-meter-research.md)) |

## Data the radar needs, and what is known about each

| Needed | Why | Known source | Evidence | Confidence |
|---|---|---|---|---|
| Player position | Radar centre | None | Aether parses no position packet | — |
| Player heading | Heading-up mode | None | — | — |
| Cube spawn event with a stable id and position | Adds a marker | None | Aether's `41 36` "entity spawn" yields id + mob code; its documented layout has unparsed gaps and **no** documented position | Unverified. Not assumed |
| Cube despawn / collection event | Removes a marker | None | Aether reads HP and deaths for combat targets only | — |
| Periodic presence confirmation | Keeps a marker from going stale | None | — | — |
| Zone/instance change | Invalidates ids | Used by packet capture: a new game connection resets the radar | Aether: a new game connection = zone change; entity ids are per connection | Documented by Aether for combat entities |
| Coordinate frame (axes, units, origin) | Correct projection | None | — | — |

## What is deliberately not done

* **No cube or position packet layout is built in or guessed.** Only the
  stream framing is implemented, as Aether documents it (verified there
  against global recordings, not yet by this project). Nothing verified shows
  which packet carries cube coordinates, so that comes from a decoder profile
  you derive from your own recordings.
* **No static map data is shown.** Aether's old map used a survey of
  "hidden cube" locations from AION2 Hub. Those are possible spawn locations,
  not live cubes, and showing them would break requirement 7.
* **No memory reading, injection, or traffic modification.** Packet capture
  is passive (Npcap, receive-only) and records only the game connection.

## What it would take to support a real source

Any tool that can **legitimately** observe cube state can feed the radar, if
its output meets this checklist:

1. **Authorized.** Permitted by the game's terms of service and by whoever
   owns the data. Don't assume this; check it.
2. **Spawn/despawn evidence.** It must distinguish "spawned now" from "a place
   cubes can spawn". If it cannot, send nothing for cubes; the radar will show
   none, which is correct.
3. **Stable ids** for the lifetime of a zone, so despawns match spawns.
4. **Presence re-confirmation** at least once per stale timeout (`cube_seen`).
5. **Known coordinate frame**: which axis is up, units, origin, heading
   convention. Configure it in Settings → *Source coordinate frame*, and check
   it against known landmarks before trusting it.
6. **Validation run**: record a session to `.jsonl`, replay it, and compare
   the radar against in-game observation, the same way Aether checks its
   damage numbers against boss HP.

Until a source passes that checklist, the shipped default is the synthetic
simulator, and the radar labels it **SYNTHETIC** at all times.

## Game-version notes from the DPS-meter research (for reference)

From Aether 3.1.0 (2026-10-14): the global service (Early Access
2026-09-30, launch 2026-10-05) speaks the same protocol as Taiwan, and Korea
matches Taiwan on every opcode Aether uses. That covers **combat** opcodes
only and says nothing about position or cube data. No client build numbers
are documented anywhere.
