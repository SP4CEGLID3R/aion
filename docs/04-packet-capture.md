# Packet capture: from your game traffic to cubes on the radar

The radar reads AION 2's traffic directly: passively, from your own PC, the
same way the Aether DPS meter does. **The built-in decoder already knows the
hidden-cube packets** for the global client (build 25800038): see
[05-aion2-protocol.md](./05-aion2-protocol.md). Choose *Packet capture*,
keep the decoder on *AION 2 hidden cubes (built-in)*, and you're done.

The rest of this guide is for when that isn't enough: a zone whose cube
type isn't known, or a game patch that moved the opcodes. It shows how to
find packets from your own sessions and either fix `aion2-decoder.json` or
write your own decoder profile.

> [!WARNING]
> Using any third-party tool with AION 2 may break NCSoft's terms of service
> and can get your account banned. Passive capture doesn't touch the game,
> but that is no guarantee. You run this at your own risk.

## What it does and doesn't do

* **Does:** copy packets from your network adapter (Npcap, receive-only,
  non-promiscuous, `tcp` filter). It finds the game connection by its
  heartbeat, rebuilds the TCP stream, splits it into game packets (framing
  documented by Aether; see `crates/radar-capture/src/framing.rs`), counts
  packet types, records, and decodes with a profile you supply.
* **Doesn't:** send, inject, block or modify any packet, open or read the
  game's process or memory, or automate input.
* **Shows cubes only** when a decoder profile marked `"validated": true`
  decodes them, and then only as long as the spawn/seen/despawn rules of the
  radar allow (see the main README).

## 0. Install

* **Npcap** from <https://npcap.com> (the app tries
  `C:\Windows\System32\Npcap\wpcap.dll`, then `wpcap.dll`). The default
  install doesn't need administrator rights at capture time. If you ticked
  "Restrict Npcap driver's access to Administrators only", run the radar as
  administrator.
* The radar app (installer), and `radar-capture.exe` (the command-line
  tool; CI publishes it next to the installers, or
  `cargo build --release -p radar-capture`).

`radar-capture devices` lists adapters. You normally don't need to pick one:
"all adapters" finds the game by content, including traffic that goes through
a local proxy or accelerator on 127.0.0.1 (Npcap's loopback adapter).

## 1. Check the game connection is found

Settings → Data source → **Packet capture**, adapter *All adapters*, no
profile, **Record** on → Apply. Log in and walk around.

The **Packet capture** panel should show a *Game connection* and a growing
*Game frames* count, with a packet-type table led by `00 36` (heartbeat). If
*Game connection* stays "not found yet", the heartbeat rule doesn't hold for
your client version. That is useful to know; report it with a short recording.

Changing map or entering a dungeon opens a new game connection. The radar
treats that as a zone change and clears everything.

## 2. Record cube sightings

With recording on, play somewhere cubes appear. Press:

| Key | When |
|---|---|
| **Ctrl+Alt+V** | the moment a cube appears on screen (or comes into view) |
| **Ctrl+Alt+C** | the moment you collect one |

The keys are global, so they work with the game focused, and the radar
flashes "Marker N". The panel's buttons do the same. Aim for **10+ sightings**
in one recording, and also spend some time *without* cubes; that contrast is
what the analysis uses.

Recordings are saved under `%APPDATA%\dev.aion2radar.overlay\recordings\` as
`aion2-capture-<time>.pcap` with a matching `.markers.jsonl`. Only the game
connection is recorded, not your other traffic. The `.pcap` opens in
Wireshark.

You can also record with Wireshark (save as `.pcapng`), or with
`radar-capture record` (type `v` / `c` + Enter for markers). Then write the
markers file yourself: one line per marker, `{"t": <ms since epoch>, "kind": "cube_visible"}`.

## 3. Find the candidate packet

```
radar-capture analyze aion2-capture-1791981420.pcap
```

This ranks packet types by how consistently they appear just before and
after your markers, compared with how often they occur anyway. It prints two
tables: by opcode, and by opcode plus the first byte after it, because one
opcode can carry several kinds of message and the spawn can drown among
them. Look for a high **hit %** together with a high **lift**. Options:
`--kind cube_collected` to analyse collection markers instead, and
`--before` / `--after` (ms) for the window around each marker (default 3000
before, 1000 after).

Try it on made-up data first: `radar-capture synth demo.pcap` writes a demo
session with invented opcodes, markers and a matching profile, and
`analyze demo.pcap` finds the invented spawn packet `C1 7E / 01`.

## 4. Find the fields

```
radar-capture inspect aion2-capture-….pcap --opcode "XX YY" [--subtype ZZ] --dump 5
```

This lists constant bytes, a varint id after the opcode if there is one,
offsets that read as plausible float coordinates (with their ranges), and
offsets whose value differs in every packet (id candidates). It hex-dumps the
packets nearest to your markers. Compare spawns at different places: the
coordinate fields change with location, the id changes per cube, and
type/category bytes stay the same.

Do the same for the player's own position (try packets that are frequent
while you move and quiet while you stand still). The radar needs it for the
centre.

## 5. Write a decoder profile

```json
{
  "name": "AION 2 global — cubes (my findings)",
  "gameVersion": "client build / date you tested",
  "validated": false,
  "notes": "How you found and checked each field.",
  "messages": [
    { "opcode": "XX YY", "event": "cube_spawned",
      "when": [ { "offset": 2, "equals": "ZZ" } ],
      "fields": {
        "id": { "offset": 3, "type": "varint" },
        "x":  { "after": "id", "offset": 0, "type": "f32le" },
        "y":  { "after": "id", "offset": 4, "type": "f32le" },
        "z":  { "after": "id", "offset": 8, "type": "f32le" } } },
    { "opcode": "..", "event": "cube_collected", "fields": { "id": { "offset": 3, "type": "varint" } } },
    { "opcode": "..", "event": "player",
      "fields": { "x": { "offset": 2, "type": "f32le" }, "y": { "offset": 6, "type": "f32le" },
                  "z": { "offset": 10, "type": "f32le" } } }
  ]
}
```

* `event`: `player`, `cube_spawned`, `cube_seen`, `cube_despawned`,
  `cube_collected`.
* Offsets count from the first opcode byte. `after` makes an offset relative
  to the end of an earlier field (order: id, x, y, z, heading), for layouts
  with a variable-length varint first.
* Types: `u8 u16le u32le u64le i16le i32le f32le f64le varint`. `scale`
  multiplies a value (e.g. a u16 angle: `0.0054931640625` turns 0–65535 into
  degrees).
* `when` limits a rule to packets with given bytes, e.g. a sub-type byte, or
  an entity code that means "cube".
* Which in-game axis is north, and the units, go in Settings → *Source
  coordinate frame*.

## 6. Validate, then switch it on

1. `radar-capture decode rec.pcap --profile my.json --out events.jsonl` prints
   how many packets decoded and how many couldn't be read. Look at the
   events: do the ids repeat sensibly, do the positions look right?
2. Replay `events.jsonl` in the radar (Data source → Replay) and compare with
   what you saw in game. Or set the radar to Packet capture with the `.pcap`
   in *Replay a capture file*.
3. Check against the game live: stand near cubes and confirm the markers sit
   where the cubes are and vanish when they're collected.
4. Only then set `"validated": true`. The radar ignores an unvalidated
   profile on purpose: decoding works, but nothing is drawn.

## What limits the range

The radar can only show what the server sends your client. MMO servers
usually send nearby objects only, so expect cubes **near you**, not across
the whole map. How near depends on the game and hasn't been measured. Your
recordings will show it: compare the distance at which spawn packets arrive
with what's on screen.

## After a game patch

Opcodes and layouts can change with any patch. Re-run step 1. If the
packet table still looks sane but cubes stop appearing, repeat steps 2–6 and
update the profile (`gameVersion` helps you keep track).
