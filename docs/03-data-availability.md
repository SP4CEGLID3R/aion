# Stage 5 — Supported game versions and missing data

## Status, plainly

| Question | Answer |
|---|---|
| Does the radar read AION 2 data today? | **Yes, with packet capture:** hidden cubes in Verteron and Altgard, other zones once a cube is opened with a key, and your own position from event packets, followed in between by an estimate ([evidence](./05-aion2-protocol.md)) |
| Which AION 2 client versions are supported? | The global client, Steam build **25800038**, per the source captures. Later patches may move opcodes; they are configurable (`aion2-decoder.json`) |
| What does it run on today? | Passive packet capture with a decoder profile, the built-in synthetic simulator, replay files (`.jsonl`, `.srf`, `.pcap`, `.pcapng`), and a loopback telemetry socket |
| Can the Aether DPS meter supply the data? | **No.** It parses combat, names, summons and HP only, with no positions and no cubes, and it has no external API ([research](./01-dps-meter-research.md)) |

## Data the radar needs, and what is known about each

| Needed | Known source | Confidence |
|---|---|---|
| Cube spawn with id and position | `34 36` object spawn, cube templates 1200039 / 1200041 | Verified in Verteron by aion2-cube-radar; checked here against its captures |
| Cube opened | `35 36` reason 04 (after `38 36` / `3a 36`) | Verified |
| Cube out of range | `35 36` reason 01 | Seen for other objects; treated as "no longer verified" (stale) |
| Presence while in range | No removal while the stream is alive | Follows from the server reporting removals; the radar re-confirms every 2 s and stops if the stream stalls |
| Player position | `2a 38`, `2b 38`, `02 38`, `0e 38`, `21 36`, `23 36` | Verified, but event-driven: can be tens of seconds old while just walking |
| Player heading | Not sent by the game | — (radar is north-up) |
| Zone change | New game connection; `21 36` map id | Verified |
| Coordinate frame | cm; north = −Y | Units supported by evidence; north unconfirmed (switchable) |
| Cube templates outside Verteron | Learned at run time from the cube key | Altgard 1301199 is a single-source hypothesis, off by default |

## What is deliberately not done

* **Nothing guessed.** Only layouts with documented evidence are built in;
  single-source hypotheses are off by default.
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
