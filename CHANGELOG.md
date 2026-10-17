# Changelog

## 0.2.0 — 2026-10-15

The radar now follows you on its own: open your wings once after logging in
and the in-game minimap keeps your position up to date about ten times a
second. An arrow around your character points at the nearest cube and says
how far it is and whether it is up or down.

### Added

- **Show the radar only when a cube is on it** (Settings → Radar, or
  **Ctrl+Tab** to switch it on and off): while no cube is drawn the radar is
  invisible and clicks go through it to the game; it appears as soon as a
  cube is on it and goes 1.5 s after the last one left. Sounds and beeps work
  as usual. Overlay mode only.

- **Minimap tracking.** The game's minimap is read from the screen (like a
  screenshot) about 10 times a second and its scroll is measured, so your
  position updates between the server's own reports. Its scale is learned from
  those reports and kept for the next session.
- **Picking up again after the minimap was covered.** After the big map, a
  dialog or a black screen, tracking continues by itself if you are still near
  the same place — no need to open the wings again.
- **Live height.** Whether a cube is above or below you is worked out at every
  minimap reading, from the ground under the monsters, NPCs and players the
  server reports around you, and checked against each of your own position
  reports. While you fly, or with nothing around, it falls back to your last
  reported height and only says up or down when it is clearly so.
- **Arrow around your character** (click-through, over the game): direction
  turned with the camera, distance, and "▲ 12 m up" / "▼ 5 m down" /
  "same height". Within a few metres: "right here" and where to look.
- **Quiet beeps** while you walk to a cube (from 30 m, a little faster as you
  get closer, one short chime on arrival, silent soon after you stop going for
  it). Own volume and a Test button.
- The live minimap can be shown inside the radar.
- Settings: a picker that takes a screenshot so you can mark where the minimap
  is (if you moved it), a live preview of what is read, and a status in plain
  words ("Following you ✓").
- A desktop shortcut: a checkbox on the installer's last page, and a button in
  Settings → Radar.

### Changed

- The radar looks like a minimap with cubes: the map, you, and the cubes with
  their distances. Names, path and legend are optional (More settings).
  Settings from an earlier version switch to this look once.
- Settings: the essentials first in plain words (Game, Your position, Finding
  cubes, Radar), everything else under *More settings*. Changes apply by
  themselves.
- Reading the client's own movement packets is off by default: the game
  encrypts what the client sends.
- Choosing the game's network traffic in Settings now says, right there, that
  such tools may be against AION 2's terms of service and can get an account
  banned. The made-up demo stays the default until you choose.
- Before your first position the radar says to open your wings once; without
  Npcap it says to install it and restart the radar.

### Fixed

- The minimap picture in the radar could be missing for long stretches
  (e.g. when a cube appeared) after the zoom fix: it was drawn only while
  the position was confirmed. It is now drawn whenever it is the minimap
  (it is always centred on you), and a changed look of the arrow's disk
  (darker places) is taken on within a second of walking.

- The radar could suddenly show a hugely zoomed-in map: whatever was on
  screen at the minimap's spot (the big map, a dialog, a loading screen) or
  a minimap at a new zoom was drawn as if it were the minimap. The game's
  minimap is now drawn only while it is the one being followed at the known
  scale (its arrow disk is checked on every frame); otherwise the radar
  shows its plain grid until it is.

- With *click-through* on, the radar's settings button could not be
  clicked and there was no other way back to the settings: **Ctrl+Alt+S**
  now opens them at any time, also while the game has focus.

- When the minimap rectangle was a few pixels off the game's arrow (other
  screen sizes or HUD layouts), tracking could drift and the minimap in the
  radar sat off-centre. The arrow is now found on the minimap by itself
  while you move (about 20 s of walking), and everything is measured around
  it.
- The radar no longer draws its own player marker over the game's minimap:
  the minimap's own arrow is you. (Without the minimap picture, the marker
  is still drawn.) In Settings the point taken as you is a thin ring around
  the arrow instead of a cross over it.

- A cube that was just collected could stay on the radar for about 10 more
  seconds: its removal could look older than the latest "still there"
  confirmation and was then ignored.
- The settings window froze on Windows when opened from the radar.
- In scenes with long roads or coasts the position looked far less certain
  than it was (±100 m within a minute).
- The legend could not be switched off.
- A settings change made while the previous one was still being applied could
  be lost, and *Reset* could be undone by a pending change.
- At low minimap reading rates, a position report arriving during a brief
  tracking hiccup was ignored.

### Safety

Unchanged and checked again for this release: nothing is injected into the
game, its process and memory are never opened, nothing is sent to it (packet
capture is listen-only; the sending functions are not even loaded), and there
is no input automation. On Windows the only system calls used for the minimap
are the screen-copy (GDI) ones.

### Known limitations

- Developed and tested against synthetic data and real minimap screenshots;
  not yet verified in the live game by the developers.
- Exclusive fullscreen is not supported (use borderless or windowed).
- Up/down between position reports needs things standing around you; on
  bridges, cliffs or when alone it says less, or asks you to open the wings.
- The installers are not code-signed: Windows SmartScreen warns on first run.

## 0.1.0 — 2026-10-14

First version: transparent always-on-top radar of confirmed, currently spawned
hidden cubes from the game's own network traffic (passive capture with the
built-in AION 2 decoder), new-cube sound, synthetic / replay / local telemetry
sources for testing, Windows installers.
