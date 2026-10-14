# Architecture

```
 ┌──────────────────────── radar-ingest ─────────────────────────┐
 │                                                               │
 │  DataSource (own thread)                                      │
 │   ├─ SimulatorSource   deterministic synthetic world          │
 │   ├─ ReplaySource      .jsonl  |  .srf / .srf.hex  ──► srf.rs  │
 │   └─ LocalTcpSource    127.0.0.1 JSON lines                    │
 │        │  WireEvent (Radar Event Protocol v1, wire.rs)         │
 │        ▼  bounded channel (8192)                               │
 │  processing thread                                            │
 │   ├─ EventAdapter      source frame → radar frame,             │
 │   │                    provenance tagging (adapter.rs)         │
 │   ├─ radar_core::Tracker   live-cube state machine             │
 │   └─ every 1/updateHz: tick() → frame() → FrameSink            │
 └───────────────────────────────────────────────────────────────┘
          │ RadarFrame (only SPAWNED, or STALE if asked)
          ▼
 ┌──────── src-tauri ────────┐        ┌─────────── src/ (TypeScript) ───────────┐
 │ TauriSink: emit_to(radar) │ ─────► │ radar.ts   interpolate, rAF loop          │
 │ commands: settings, debug │        │ layout.ts  what to draw (tested)          │
 │ windows: radar, settings  │ ◄───── │ projection.ts  world → screen (tested)    │
 │ store.rs: settings.json   │        │ renderer.ts    canvas painting            │
 └───────────────────────────┘        │ settings.ts    form + diagnostics panel  │
                                      └──────────────────────────────────────────┘
```

## Layers and where they live

| Layer (requirement 5) | Code | Tests |
|---|---|---|
| 1. Data-source adapters | `crates/radar-ingest/src/source.rs`, `adapter.rs` | unit tests in both; TCP loopback test |
| 2. Event ingestion and parsing | `wire.rs` (protocol), `srf.rs` (synthetic byte-stream parser) | malformed input, chunking, resync, fixtures |
| 3. Entity and coordinate state | `crates/radar-core/src/tracker.rs`, `geometry.rs` | 6 acceptance tests + 14 more; transform tests |
| 4. Radar projection and rendering | `src/lib/projection.ts`, `layout.ts`, `renderer.ts` | vitest |
| 5. Settings and logging | `radar-core/src/settings.rs`, `src-tauri/src/store.rs`, `tauri-plugin-log` | sanitising, round-trip, corrupt-file recovery |
| 6. Automated tests | `cargo test --workspace`, `pnpm test`, CI | — |

## Design decisions

**The tracker is the only judge of "live".** Sources and adapters pass
claims along with their provenance; they never decide what is shown. The
renderer applies the same filter again (`layout.ts` drops anything but
`SPAWNED`/`STALE`), and `Tracker::frame` re-checks ages itself, so a missed
timer tick cannot make an old cube look live.

**Static and historical data travels through the pipeline, tagged.** The
adapter turns `spawn_point` and `historical_location` into cube events with
`StaticSpawnPoint` / `Historical` provenance instead of dropping them. The
tracker rejects them, and the rejection is counted and logged, so the debug
panel shows that such data arrived and was refused.

**Time is explicit.** Every core function takes `now`. Tests drive the
clock, and replay rebases recordings to the present (an idea taken from
Aether's recorder).

**Zone changes reset everything.** Aether found that entity ids are per
connection: the same character had one id in town and another in a dungeon.
Ids are not assumed to survive a zone change, here or in any future source.

**Parser isolated from overlay.** `srf.rs` and `wire.rs` produce plain
`WireEvent`s and know nothing about the tracker or UI. The tracker knows
nothing about bytes. Each is tested alone, and `tests/acceptance.rs` tests
the chain.

**UI never blocks.** The source and processing threads are separate. The
tracker mutex is held for microseconds. Frames are pushed to the webview at
`updateHz`, and the canvas draws at display rate, interpolating the player
between frames. If frames stop arriving, the renderer clears the markers
rather than freezing the last ones on screen.

**Windows.** The radar window is created transparent and borderless, always
on top, and does not take focus. *Separate window* mode turns decorations
on and always-on-top off at runtime. Transparency can't change after
creation, so that mode paints an opaque background instead. Click-through
uses `set_ignore_cursor_events` and is switched off again from the settings
window.
