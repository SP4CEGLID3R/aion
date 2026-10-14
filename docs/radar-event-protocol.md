# Radar Event Protocol v1

The structured format the radar accepts from any data source. A telemetry
tool, a parser, or a file uses it to tell the radar what is happening. The
radar decides what to show (see *Rules* below).

Implemented in [`crates/radar-ingest/src/wire.rs`](../crates/radar-ingest/src/wire.rs).
A reference producer is in [`tools/telemetry_demo.py`](../tools/telemetry_demo.py).

## Transport

* **JSON lines**: one JSON object per line, UTF-8, `\n`-terminated.
* Lines longer than 16 KiB are rejected. Blank lines and lines that start
  with `#` are ignored.
* **Live**: TCP on `127.0.0.1:<port>` (default `47810`). The socket binds to
  loopback only and can't be reached from the network. One client at a time;
  when it disconnects the radar goes back to listening. There is no
  authentication: any process on the same machine can connect.
* **Replay**: a `.jsonl` file holding the same lines. Timestamps are shifted
  so that the first event plays "now", and relative timing is kept
  (optionally sped up).

## Envelope

Every line has `"v": 1` and a `"type"`. Unknown types, a missing or different
`v`, or missing required fields reject that line only. The connection stays
open, and rejections are logged and counted.

Common fields:

| Field | Type | Meaning |
|---|---|---|
| `t` | integer | Observation time, **ms since the Unix epoch**, on the same machine clock as the radar |
| `id` | string or integer | **Stable** identifier of the cube for the lifetime of the zone |
| `pos` | `[x, y, z]` numbers | Position in the **source's** coordinate frame (see *Coordinates*) |
| `zone` | string | Optional zone/instance name |
| `label` | string | Optional display name |

## Event types

| `type` | Required | Optional | Effect |
|---|---|---|---|
| `player` | `t`, `pos` | `heading` (degrees), `zone` | Moves the radar centre. A different `zone` than the current one counts as a zone change |
| `cube_spawned` | `t`, `id`, `pos` | `label`, `zone`, `provenance` | Confirms the cube **has spawned and is present**. Adds a marker |
| `cube_seen` | `t`, `id` | `pos`, `zone`, `provenance` | Re-confirms a spawned cube is still there. Resets its stale timer |
| `cube_despawned` | `t`, `id` | `zone` | Confirms the cube is gone. Removes its marker |
| `cube_collected` | `t`, `id` | `zone` | Confirms the cube was collected. Removes its marker |
| `zone_changed` | `t`, `zone` | | Drops every tracked cube; ids are not assumed to survive a zone change |
| `spawn_point` | `pos` | `t`, `id`, `label`, `zone` | Static reference data. **Never shown.** Counted for diagnostics |
| `historical_location` | `t`, `pos` | `id`, `label`, `zone` | A past location. **Never shown.** Counted for diagnostics |

`provenance` is `"live"` (default), `"historical"` or `"predicted"`. Only
`live` can create a marker.

### Examples

```json
{"v":1,"type":"zone_changed","t":1760000000000,"zone":"demo-zone"}
{"v":1,"type":"player","t":1760000000000,"pos":[0,0,0],"heading":90}
{"v":1,"type":"cube_spawned","t":1760000000100,"id":"c-17","pos":[12.5,40,3],"label":"Cube"}
{"v":1,"type":"cube_seen","t":1760000001100,"id":"c-17"}
{"v":1,"type":"cube_collected","t":1760000004000,"id":"c-17"}
{"v":1,"type":"spawn_point","id":"sp-3","pos":[60,-60,0]}
```

## Rules the radar applies

These are enforced in [`radar-core/src/tracker.rs`](../crates/radar-core/src/tracker.rs):

1. Only `live` observations change state. `spawn_point`, `historical_location`
   and `provenance: historical|predicted` are rejected.
2. A cube without an `id` is rejected. Without an id, a later despawn could
   not be matched to it.
3. A marker appears only after `cube_spawned` with a finite `pos`. A
   `cube_seen` for an unknown id leaves it `UNKNOWN`, which is never drawn.
4. `cube_despawned` / `cube_collected` remove the marker and leave a
   tombstone. A later event **with an older `t`** for that id is rejected,
   so it cannot bring the cube back.
5. A spawned cube with no `cube_spawned`/`cube_seen` within the stale timeout
   (default 10 s) becomes `STALE`. Stale cubes are hidden by default, or drawn
   hollow and labelled STALE if the user chooses that.
6. Per id, events must not go back in time. Events stamped more than 2 s in
   the future, or presence events older than the stale timeout, are rejected.
7. Repeating a spawn for an already-spawned id is a duplicate. It refreshes
   the cube's timers and never adds a second marker.

**Producers should send `cube_seen` for every present cube at least once per
stale timeout.** A producer that only sends spawn/despawn will see its cubes
go stale. That is intended: the radar does not assume a cube is still there.

## Coordinates

The radar's world frame is **x = east, y = north, z = up**, in arbitrary
"world units", with compass headings (0 = north, clockwise). A source in any
other frame is mapped by the **coordinate transform** in Settings → *Source
coordinate frame*:

* which signed source axis is east / north / up (any permutation; reflections
  are allowed, so a Y-down image frame is handled by mapping north to `−Y`)
* scale (world units per source unit) and origin
* heading offset and direction (clockwise or counter-clockwise)

Nothing about AION 2's own coordinate system is assumed; see
[03-data-availability.md](./03-data-availability.md).

## Versioning

Breaking changes would use `"v": 2`. A v1 radar rejects other versions line by
line rather than guessing.
