# Following the player with the minimap

The server reports the player's own position only with events (opening the
wings, buffs, being attacked, loading screens). A tool that waits for those
is "stuck" for seconds to minutes at a time. This page describes how the
radar fills the gaps from the game's own minimap, and why other sources do
not work.

## Why not packets

* **Client → server is encrypted.** Everything the client sends after the
  handshake is enciphered (RC4 under a key negotiated with RSA per
  connection); only the length prefix is readable. The own movement is in
  there (about 10 packets per second while moving), but recovering the key
  takes reading the game's memory, which this project never does. Source:
  lunixxx37/aion2-protocol-research (`docs/C2S_RESEARCH.md`), and every
  other public AION 2 tool reads only server → client traffic. The radar's
  learner for client movement (`aion2/movement.rs`) is therefore off by
  default.
* **Server → client has no frequent own position.** A scan of every frame
  of real captures found the own position only in event records
  (aion2-cube-radar `docs/FINDINGS.md`, P1–P3).
* **Objects entering and leaving range** give "you are about R from here"
  measurements (`aion2/edge.rs`). That works as a fallback once the radii
  are known, but learning them needs frequent position fixes, which is the
  very thing that is missing. With the minimap it learns them quickly.

## How the minimap is read

The minimap (top right, below its title bar) is **north-up** and **centred
on the player**: the map scrolls under a fixed arrow. Facts from public
tools: madara134/aion2-cube-finder (MIT), NoRhythmDEV/aion2-map-overlay,
NewAge-BD/Soulcrest; checked on their screenshots.

* **Screen copy** (`crates/radar-vision/src/capture.rs`): GDI `BitBlt` of
  one rectangle of the desktop, about 10 times a second. No handle to the
  game, no hooks. Exclusive fullscreen comes out black (detected and
  reported); borderless or windowed works. The radar and the guide arrow
  mark themselves `WDA_EXCLUDEFROMCAPTURE`, so they never appear in the copy
  even when placed over the minimap.
* **Movement** (`tracker.rs`, `ncc.rs`): each frame is band-passed (box blur
  3 minus 13: map detail stays, the view cone's tint goes) and matched
  against a keyframe with masked normalised cross-correlation, computed with
  FFTs for all shifts at once (Padfield 2012). Masked out: the arrow disk,
  the two bright view-cone edge lines, the cyan range ring, the corner HUD
  icons, white and red dots (other players, NPCs), the border. Sub-pixel
  peak by alternating row/column parabolas. Keyframes are kept up to 3 s
  (fewer re-anchors, less drift) and replaced after a large move or a weaker
  match. A match whose peak is a ridge (one road or coast fills the view) is
  flagged: movement along it cannot be measured.
* **Finding the arrow**: the rectangle's centre can be a few pixels off the
  arrow (another screen size, a slightly different HUD). While you move,
  views taken at different places are compared: around the arrow nothing
  changes (its light disk stays put), everywhere else the map scrolls. The
  point whose surroundings vary least is the arrow; once two measurements
  agree (about 20 s of walking) the pivot moves there and everything fixed
  to it (masks, view cone, range ring) is measured around it. Until then
  the masks around the centre are wider. On synthetic minimaps with the
  arrow 9–10 px off centre: found within 0.5 px; tracking error stayed
  under 1.5 px (25 px without this).
* **Camera direction**: the two bright edges of the view cone, about 114°
  apart, found in the angular brightness profile around the arrow.
* **Black or frozen capture** and **track breaks** are reported, never
  hidden.
* **Picking up again**: the last good view of each recent segment is kept
  (3 segments, 2 minutes). When the track breaks or the minimap comes back
  after being covered (the big map, a dialog, a black frame), the new view
  is matched against them (stricter: peak ≥ 0.6, margin ≥ 0.08); if it is
  the same place, the old segment continues with its own coordinates, and
  the fusion returns to that segment's position chain. So closing the big
  map does not need the wings again, as long as you did not travel further
  than about a third of the minimap meanwhile.

Measured on synthetic minimaps (procedural maps with cone, ring, icons and
moving dots; 60 s walks with turns, stops and gliding): within about 2 px
(≈ 2 m) of the true path at both 1440p and 1080p sizes, 6–13 ms per frame.
On the real minimap screenshots from aion2-cube-finder: within about 1 px
over 40 s, and the view cone found on both (strength 0.6–0.8; 0.3 needed).
Not yet measured in the live game.

## From minimap pixels to the world

`crates/radar-capture/src/aion2/minimap.rs`:

* **Anchor at every server fix**; in between, position = last fix + minimap
  movement since then, converted with the scale and axis mapping.
* **Scale**: seeded at 1.03 px per metre at the default UI on a 1440p screen
  (scaled with the screen height), then learned from "legs": the world
  distance between two fixes (10–300 m apart, at most 60 s) against the
  minimap distance over the same time; median of the last 9. A leg that
  disagrees by more than 25 % (or a changed range ring) starts a re-fit:
  two agreeing legs set the new scale, so a minimap zoom change is handled.
* **Axes**: seeded "right = +X, down = +Y" (north = −Y), re-fitted from leg
  directions among the 8 possible mappings once 4 legs agree.
* **Uncertainty** (the dashed circle): fix error, 3 % of the distance moved
  (12 % before the scale is confirmed), extra for bridged breaks and for
  plain stretches (a road or coast filling the view: up to running speed
  for a flat match peak, in proportion for an elongated one). When unsure (minimap hidden, track broken, map not moving with
  you), the radar shows the last fix rather than a guess.
* The calibration is saved in `aion2-learned.json` and reused.
* **Height**: the minimap is flat, so it says nothing about height. Each
  position between fixes gets the better of two height estimates, with its
  uncertainty: the last fix's height (0.5 m, plus 1 m per 5 m moved from
  the fix: ground rising or falling by up to ~11°), or the ground model
  (`aion2/ground.rs`): a robust, distance-weighted plane through the
  positions the server sends for everything within 25–40 m (objects,
  monsters, NPCs, other players), checked against every own fix (its
  typical miss adds to the uncertainty; before two checks, 3 m is
  assumed). A fix more than 3 m above the modelled ground means flying or
  gliding: the ground model is then not used until the next fix. The guide
  only says a cube is up or down when the difference is at least twice the
  uncertainty (`src/lib/guide.ts`, `heightCue`).

Order of sources: a fresh server fix, then the minimap, then the
object-range estimate.

## Setting it up

Usually nothing: the minimap's default spot on the primary monitor is used.
If the HUD was edited, Settings → *Your position* → *Pick the minimap on
screen…* takes a screenshot (the settings window steps aside) and lets you
drag a box over the map area. The preview shows exactly what is read, with
a ring around the point taken as you (the pivot).

## Credits

The tracking approach and its constants follow madara134/aion2-cube-finder
(MIT, © 2026 dssert): see `crates/radar-vision/NOTICE`. This is an
independent Rust implementation. No game assets are included.
