#!/usr/bin/env python3
"""Reference producer for the Radar Event Protocol v1 (synthetic data).

Connects to the radar's local telemetry socket and plays a short, made-up
scenario that exercises every rule the radar enforces:

  * cube-A  spawns and is re-confirmed every second       -> live marker
  * cube-B  spawns once and is never confirmed again      -> STALE after the timeout
  * cube-C  spawns, then is collected after 6 s           -> marker removed
  * sp-1    is a static spawn point                       -> never shown
  * old-1   is a historical location                      -> never shown

Use it to check an installation, or as a template for an authorized
telemetry tool. Standard library only.

    python tools/telemetry_demo.py [--port 47810] [--seconds 30]

In the radar settings choose "Local telemetry on 127.0.0.1" with the same port.
"""

import argparse
import json
import math
import socket
import time


def now_ms() -> int:
    return int(time.time() * 1000)


def line(**event) -> bytes:
    return (json.dumps({"v": 1, **event}) + "\n").encode()


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", type=int, default=47810)
    ap.add_argument("--seconds", type=float, default=30.0)
    args = ap.parse_args()

    with socket.create_connection(("127.0.0.1", args.port), timeout=5) as s:
        t = now_ms()
        s.sendall(line(type="zone_changed", t=t, zone="demo-zone"))
        s.sendall(line(type="player", t=t, pos=[0, 0, 0], heading=0, zone="demo-zone"))
        s.sendall(line(type="cube_spawned", t=t, id="cube-A", pos=[30, 40, 0], label="Cube A"))
        s.sendall(line(type="cube_spawned", t=t, id="cube-B", pos=[-50, 20, 0], label="Cube B"))
        s.sendall(line(type="cube_spawned", t=t, id="cube-C", pos=[10, -35, 0], label="Cube C"))
        s.sendall(line(type="spawn_point", id="sp-1", pos=[60, -60, 0], label="Possible spawn"))
        s.sendall(line(type="historical_location", t=t - 60_000, id="old-1", pos=[-20, -70, 0]))

        start = time.time()
        collected = False
        while time.time() - start < args.seconds:
            t = now_ms()
            a = (time.time() - start) / 8
            s.sendall(line(type="player", t=t, pos=[8 * math.sin(a), 8 * math.cos(a), 0],
                           heading=(math.degrees(a) + 90) % 360))
            s.sendall(line(type="cube_seen", t=t, id="cube-A"))
            if not collected:
                s.sendall(line(type="cube_seen", t=t, id="cube-C"))
            if not collected and time.time() - start > 6:
                s.sendall(line(type="cube_collected", t=t, id="cube-C"))
                collected = True
            time.sleep(1.0)


if __name__ == "__main__":
    main()
