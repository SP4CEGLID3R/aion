# AION 2 hidden cubes on the wire

What the built-in decoder (`crates/radar-capture/src/aion2.rs`) reads, where
each fact comes from, and how sure it is.

## Sources reviewed (2026-10-14)

| Repository | What it is | Useful for the radar? |
|---|---|---|
| [dennisreichenberg/aion2-cube-radar](https://github.com/dennisreichenberg/aion2-cube-radar) (GPL-3.0, commit `83f0067`) | A working passive cube radar with a detailed findings log (`docs/FINDINGS.md`) built from its author's own captures, plus real stream fixtures | **Yes, the source of every layout below.** Facts re-implemented here independently; no code or captured data copied |
| [filvoch/AionCoreUpdate](https://github.com/filvoch/AionCoreUpdate) | Installer and README of the closed-source "AionCore" app, which has a cube radar | Confirms independently that cubes are sent by the game and that the own position only arrives with skills/dashes. No protocol details (source not public) |
| [Kloggzer/the-completionist-aion2](https://github.com/Kloggzer/the-completionist-aion2) (MIT) | An overlay with maps and **possible** cube spots from aion2maps.com. Its packet reading was **removed** in v2.0 | No: static spawn spots only, which the radar must never show as live cubes. Its README says NCSoft prohibits software that intercepts or analyses game traffic |

## Facts the decoder relies on

Client: global, Steam build **25800038** (2026-10-09). Opcodes are the two
bytes on the wire; bodies follow them; numbers are little-endian; varints
are LEB128.

| Packet | Meaning | Layout used | Evidence (aion2-cube-radar) |
|---|---|---|---|
| `00 36` | Heartbeat, 20 per second | 8-byte body | Verified (B3) |
| Framing | `varint len, [flag F0–FE], payload`; frame = len + varint bytes − 4; `FF FF` = LZ4 bundle | — | Verified (B2, B4); same as the Aether DPS meter documents |
| `34 36` | Object spawn | id varint, kind u16, **template u32**, **x y z f32**, f32, heading f32, 3 B, created u64 ms | Verified (B5, B9) |
| — | **Hidden cube** = spawn with template **1200039** or **1200041** (Verteron) | — | Verified: 5 cubes, positions equal known cube spots to the unit |
| — | Template 1301199 (Altgard?) | — | **Hypothesis**, a single third-party source. Off by default; Settings → *Also show unconfirmed cube types* turns it on, and its markers read "Hidden Cube? (…, unconfirmed type)" |
| `35 36` | Object removed | id varint, reason u8: **04** opened, **01** left the send range | 04 verified for cubes (B6); 01 seen for other objects (O2) |
| `38 36` / `3a 36` | Opening starts / finishes | object varint, actor varint, … / count u32, item u32 | Verified (B6) |
| — | Cube key | `3a 36` ends with count 1, item `0x259AC523` | Seen on all 3 cube openings. Used only to *learn* new cube templates |
| `2a 38`, `2b 38` | Buff records with the **caster's** position | caster varint before the trailing x y z | Verified (B7, P2) |
| `02 38` kind 2, `0e 38` | Records with the **target's** position | see `aion2.rs` | Verified (P2) |
| `21 36`, `23 36` | Map load, teleport: own position | x y z at 16 / at 6; map id u32 at 4 | Verified (P7), map 1010 = Verteron |
| `33 36` | Own character (first varint = own id) | — | Community; also documented by the Aether DPS meter |
| Units | 1 unit = 1 cm | — | Supported: cube openings happen ~2 m away (B7) |
| North | Map north = world −Y | — | **Unconfirmed**; switchable in Settings |
| Send range | Cubes arrive about **97–105 m** away (planar) | — | Measured 4 times (B8) |

## Checked against real traffic

The decoder in this repository was run (locally, not committed) over the
real stream fixtures published in aion2-cube-radar:

* **Framing:** 207/207 and 53/53 frames identical to the reference,
  including an LZ4 bundle of 31 inner frames, 0 resyncs.
* **Cubes:** all three cubes in the fixtures (ids 65509, 34438, 55235;
  templates 1200039 and 1200041) were found at exactly the recorded
  positions, and each was reported collected when opened.
* **Own character:** id 9180 identified from self-buffs.
* **Positions:** 27 of 27 position records (including records that must be
  rejected) decoded exactly as expected.

Not checked: the live game itself (there is no game client in the
development environment), zones other than Verteron, and any client build
after 25800038.

## How the radar uses it

* **Live:** a `34 36` with a cube template adds a marker.
* **Still there:** the server sends `35 36` when an object leaves range or is
  opened. While the game stream keeps arriving (heartbeats) with no removal,
  the cube is confirmed again every 2 s. If the stream stops or bytes are
  lost, confirmations stop and every cube becomes stale. Nothing is shown as
  live on a guess.
* **Opened (04):** removed. **Left range (01):** stale (hidden by default).
* **New connection / new map:** everything is cleared, because ids are per
  connection.
* **Other zones:** when you open an object with the cube key, its template is
  learned for the rest of the session and later spawns of it are shown. The
  panel lists learned templates; add them to `aion2-decoder.json` to keep
  them.
* **Your position** comes only with events (your buffs, being attacked,
  loading screens), as all three projects report. The radar shows its age.
  The game does not send which way you face, so the radar is north-up.

## After a game patch

Opcodes can move. Copy `crates/radar-capture/src/aion2-default.json` to the
app's config folder as `aion2-decoder.json`, fix the opcodes or templates,
and restart. The capture panel warns when heartbeats or object spawns stop
arriving. `radar-capture census` and `analyze` (see
[04-packet-capture.md](./04-packet-capture.md)) find the new values.

## Terms of service

The Completionist's author reports that NCSoft support called software that
intercepts or analyses game traffic a terms-of-service violation, and removed
that feature for it. aion2-cube-radar repeats this, and AionCore warns that
no tool can guarantee how NCSoft will react. Using this can get an account
banned.
