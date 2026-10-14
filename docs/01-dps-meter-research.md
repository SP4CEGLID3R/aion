# Stage 1 — Research: Aether AION 2 DPS meter

Subject: <https://github.com/Helveticxa/Aether-Aion2-DPS-meter-Global>
Inspected revision: `689d521` (tag 3.1.0, committed 2026-10-14). The repository was
cloned read-only to a scratch directory; **no code from it is copied into this
project** (see *License* below).

Everything in this document is either quoted from that repository or marked as
an inference. Where something could not be verified it is listed under
*Uncertainties*.

---

## 1. What it is

| Item | Finding | Evidence |
|---|---|---|
| Purpose | Real-time DPS meter for AION 2, global servers | README |
| Method | **Passive** packet capture (Npcap, or WinDivert in sniff/receive-only mode). States it never injects code, opens the game process, reads/writes memory, or modifies packets | README "What Aether never does" |
| Stack | Rust backend + Tauri 2 + React 19 / Vite / Tailwind frontend | `src-tauri/Cargo.toml`, `package.json` |
| Platform | Windows 10/11, must run as Administrator (capture needs it) | README "Install", FORK.md "Running" |
| Ancestry | Fork of NOIA2 (zdyoung), which wrote the capture pipeline and parsers. Upstream froze its public source in October 2026 | README "Credit", FORK.md |
| License | **GPL-3.0-only** (inherited from NOIA2). "Any distributed build must ship its source under the same terms." | `LICENSE`, `Cargo.toml`, README |

## 2. Supported game versions

| Service | Status claimed by the project |
|---|---|
| Global (Early Access 2026-09-30, launch 2026-10-05) | "Built and tested on the live global service"; checked against recorded sessions since 3.0.0 |
| Taiwan | Same protocol as global, per recordings (FORK.md "On Early Access day") |
| Korea | Opcodes said to be identical across Korea and Taiwan "verified by comparing all three open implementations" |

There is **no game client build number** recorded anywhere. Support is
described per *service*, not per client patch. Any patch can change opcodes;
the project's own answer to that is its packet recorder and "opcode census"
tooling, not a version table.

## 3. Architecture of the packet pipeline

Reconstructed from `src-tauri/src/dps_meter/`:

```
capture/capturer.rs, windivert_capturer.rs   Npcap / WinDivert sniffing -> CapturedPacket
        │  (bounded Channel)
capture/dispatcher.rs                         per-TCP-flow sorting; a flow becomes "the game's"
        │                                     after two heartbeats 0E 00 36 within 3 s;
        │                                     a new game connection = zone change
capture/processor.rs                          framing: [length varint][optional extraFlag][payload];
        │                                     FF FF payload = LZ4-compressed bundle of frames;
        │                                     resync at the next heartbeat on loss
capture/parser/*.rs                           opcode -> parser (damage, dot, buff, summon, nickname)
        │  (callbacks registered by the engine)
storage/data_storage.rs                       entity / fight state keyed by per-zone entity id
        │
engine/meter.rs                               snapshot thread: builds CombatSnapshot only when
                                              data changed, emits Tauri event "dps-snapshot"
                                              to its own overlay webviews
```

Notable techniques worth learning from (architecture, not code):

* **Capture → channel → dispatcher → parser → storage → snapshot** with a
  bounded channel between each thread, so the UI never blocks on capture.
* **Snapshot rebuilt only when something changed**, sent only when something
  shown changed (FORK.md "Cost").
* **Entity ids are per connection/zone** — the same character was `9231` in
  town and `802` in a dungeon (FORK.md "Reading the game's stream"). Anything
  keyed by id must be discarded on a zone change.
* **Record and replay**: raw packets are written to a `.aetherpc` file
  (`capture/recorder.rs`: `"AETHERPC"` magic, u16 version, then per-record
  timestamp, IPs, ports, sequence, length, bytes) and replayed through the
  identical code path, shifted to the present.
* **Opcode census**: counts every opcode with size ranges and whether a parser
  claims it — the project's way to detect protocol drift instead of assuming.

## 4. Packet types the project actually parses

From `capture/processor.rs` dispatch and
`docs/AION2_PACKET_PROTOCOL_ANALYSIS.zh-CN.md` (upstream's notes, Chinese):

| Opcode | Meaning (per the project) | Contains position? |
|---|---|---|
| `0E 00 36` | Heartbeat (~20/s), used for flow detection and resync | No |
| `33 36` | Own character name + id, on loading screens | No |
| `4A 36` | Own entity id (3.1.0) | No |
| `45 36` | Other players' names as they come into view | No |
| `41 36` | Summon / **entity spawn**: `actor_id -> mobCode`, `summon -> owner` | **Not documented.** The documented layout has `[unknown ...]` gaps; only id, mob code and owner are extracted |
| `04 8D` / `07 02 06` / `07 02 01` | Summon ownership | No |
| `04 38`, `05 38` | Direct damage, DoT | No |
| `2A 38`, `2B 38` | Buffs | No |
| `00 8D` | Remaining HP | No |

The structured result types (`models/packet.rs`, `models/combat.rs`) are combat
records: `ParsedDamagePacket { target_id, actor_id, skill_code, damage, ... }`.
**No struct, parser, or field anywhere in the source handles coordinates,
movement, player position, or cubes** (searched for `position`, `coord`,
`pos_x`, `cube`, `spawn`, `move`, etc.; also `git log -S` over the 50 fetched
commits).

## 5. Is there a reusable parser or an external interface?

| Candidate | Exists? | Notes |
|---|---|---|
| Library crate / published package | No | `aether_lib` is the app's own lib target, not published, Windows-only deps (WinDivert) |
| Plugin interface | No | Parsers are wired with internal callbacks (`dispatcher.rs`, `engine/meter.rs`) |
| Exported event stream (socket, pipe, file) | No | Events go only to its own webviews via Tauri IPC (`dps-snapshot`, `history-updated`, …) |
| Documented structured-event format | No | Only the **raw-packet** `.aetherpc` recording format is documented |
| HTTP / WebSocket server | No | None found |

## 6. The removed map, and why it does not help

Versions 0.1.6–2.1.x had an interactive map and always-on-top minimap
(`CHANGELOG.md` 0.1.6; removed in 2.2.0). Its markers — including a
**"hidden cubes"** category — were a **static marker database** from
[AION2 Hub](https://aion2hub.com/maps), imported from a file
(`public/aion2/maps/SOURCE.md` at commit `966ab73`). They were survey
locations, not anything read from packets, and the map showed no live player
position.

That is exactly the kind of data this project's requirement 7 forbids
presenting as live: static spawn points.

## 7. Reuse decision

| Component | Decision | Reason |
|---|---|---|
| Any source code | **Not reused** | GPL-3.0-only would bind this project to GPL. Also nothing in it produces the data a radar needs |
| Packet parser | **Not reused** | Parses combat only. Reusing it would also mean capturing game traffic, which this project avoids until a source with documented positions exists |
| Pipeline shape (bounded channels, snapshot-on-change, zone reset, record/replay, census) | **Used as architectural reference** | Implemented independently here: `radar-ingest` pipeline, `ZoneChanged` reset, JSONL/SRF replay, per-reason rejection counters |
| Tauri overlay approach (transparent, non-focusable, always-on-top webview) | Reference | Implemented independently in `src-tauri/` |
| Static map markers | **Rejected** | Static spawn points, forbidden by requirement 7 |

## 8. Consequence for the radar

The DPS meter **cannot** supply cube spawn/despawn events or coordinates, nor
player coordinates. So this project:

1. Defines its own documented, versioned **Radar Event Protocol v1** (JSON
   lines; [`radar-event-protocol.md`](./radar-event-protocol.md)) that any
   *authorized* tool can produce over a loopback socket or a file.
2. Ships a **synthetic** binary frame format (SRF) and parser to develop and
   test the parser → adapter → state → render chain without touching game
   traffic. SRF is fictional and is *not* AION 2's protocol.
3. Ships a deterministic synthetic simulator as the default data source.
4. Contains **no AION 2 packet structures**: none has been verified to carry
   cube or player coordinates.

See [`03-data-availability.md`](./03-data-availability.md) for what is missing.

## 9. Uncertainties

* Only the last 50 commits were fetched. Older history was not searched for
  position parsing (the earliest fetched commit is upstream's map/boss timer
  commit `3ba276b`).
* The `41 36` spawn packet has undocumented regions. It *might* contain a
  position; nothing in the project shows it does, and this project does not
  assume it.
* No information on whether NCSoft's terms of service permit this or any
  overlay. Aether's own README: "Nobody here can promise how NCSoft will treat
  any particular tool."
* Whether the global client keeps the opcodes listed above after future
  patches is unknown; the project itself relies on census tooling to find out.
