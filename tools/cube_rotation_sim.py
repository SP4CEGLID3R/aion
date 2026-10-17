#!/usr/bin/env python3
"""Monte Carlo comparison of hidden-cube farming rotations (AION 2).

The world follows what is known about hidden cubes (see docs/07-cube-patterns.md):

* The zone's cube groups (Altgard 59, Verteron 95), grouped into areas by
  the counts guides give (densest: Altgard 6, Verteron 8-9 groups); at most
  one cube per group at a time. Areas take longer to sweep the more spots
  they hold (Altgard about 23 per group, Verteron about 15).
* An opened cube's group spawns again after a delay. The delay is unknown:
  each simulated session draws its own median (log-uniform 5-90 min), each
  group varies around it and every respawn varies again. In Verteron, Ellun
  River Midstream waits 1.5x as long (data-mined 5400 s vs 3600 s).
* Other players open cubes too. How fast is unknown: each session draws the
  mean time an available cube survives (log-uniform 20 s - 20 min; in real
  captures fresh cubes were taken in 18-27 s and cubes found on arrival were
  1-3 min old), and how much
  more crowded the dense areas are (gamma 0 = no more, 1 = in proportion to
  their cubes).
* Whether each channel has its own cubes is unconfirmed: half the sessions
  have independent channels, half one shared set (hopping then only costs
  time).
* You see a cube with the radar while sweeping its area (it turns up at a
  random moment of the sweep), then go and open it (reaction 2-5 s +
  distance / speed + 4 s to open);
  you get it only if nobody else opens it first.

Every session draws the unknowns and a random layout of the areas, then runs
every rotation on the same world for two hours. Usage:

    python3 tools/cube_rotation_sim.py --zone altgard --sims 10000
"""

import argparse
import json
import math
import time

import numpy as np

# Groups per area, densest first. Named areas and their counts are from
# guides; the rest are unnamed areas that bring the total to the zone's
# groups (data-mined: Verteron 95 groups / 1,440 spots, Altgard 59 / 1,371).
ZONES = {
    "verteron": {
        "groups": [9, 9, 8, 8, 8, 7, 7, 7, 6, 6, 5, 5, 5, 5],
        "names": ["Altamia Canyon", "Ellun River Midstream", "Verteron Fortress Ruins"],
        "spots_per_group": 1440 / 95,
        "long_delay_area": 1,  # Ellun River Midstream: 1.5x respawn delay
    },
    "altgard": {
        "groups": [6, 5, 5, 5, 5, 4, 4, 4, 4, 4, 4, 3, 3, 3],
        "names": ["Dredgion Crash Site"],
        "spots_per_group": 1371 / 59,
        "long_delay_area": None,
    },
}
AREA_GROUPS = np.array(ZONES["altgard"]["groups"])
SPOTS_PER_GROUP = ZONES["altgard"]["spots_per_group"]
LONG_DELAY_AREA = ZONES["altgard"]["long_delay_area"]


def use_zone(name):
    global AREA_GROUPS, SPOTS_PER_GROUP, LONG_DELAY_AREA
    z = ZONES[name]
    AREA_GROUPS = np.array(z["groups"])
    SPOTS_PER_GROUP = z["spots_per_group"]
    LONG_DELAY_AREA = z["long_delay_area"]
    return z["names"] + [f"area {i + 1}" for i in range(len(z["names"]), len(z["groups"]))]

CHANNELS = 3
MAP_M = 3000.0  # the areas lie in a square of this size
MIN_AREA_GAP_M = 300.0
SESSION_S = 2 * 3600
DT = 5.0
WARMUP_S = 6 * 3600
WARMUP_DT = 30.0
GREEDY_TAU_S = 20 * 60  # the smart rotation's guess of the respawn time


def strategies():
    """name -> (areas toured, densest first | 'greedy', channels per area)."""
    return {
        "Camp densest area": ([0], 1),
        "Camp densest + 2-channel hop": ([0], 2),
        "Camp densest + 3-channel hop": ([0], 3),
        "Loop top 3 areas": ([0, 1, 2], 1),
        "Loop top 3 + 2-channel hop": ([0, 1, 2], 2),
        "Loop top 3 + 3-channel hop": ([0, 1, 2], 3),
        "Loop top 6 areas": (list(range(6)), 1),
        "Loop top 6 + 2-channel hop": (list(range(6)), 2),
        "Loop whole map": (list(range(len(AREA_GROUPS))), 1),
        "Smart: longest-unvisited, 1 channel": ("greedy", 1),
        "Smart: longest-unvisited, 3 channels": ("greedy", 3),
    }


class World:
    """Per-session parameters, shared by all rotations of that session."""

    def __init__(self, n, seed):
        rng = np.random.default_rng(seed)
        a = len(AREA_GROUPS)
        self.n = n
        self.area_of_group = np.repeat(np.arange(a), AREA_GROUPS)
        g = len(self.area_of_group)
        self.speed = rng.uniform(8, 14, n)  # m/s, mount / flying
        self.switch_s = rng.uniform(10, 30, n)  # channel change
        sweep_base = rng.uniform(60, 120, n)  # s to sweep an area of 100 spots
        spots = AREA_GROUPS * SPOTS_PER_GROUP
        self.sweep_s = sweep_base[:, None] * np.sqrt(spots / 100.0)[None, :]
        self.delay_median = np.exp(rng.uniform(math.log(5 * 60), math.log(90 * 60), n))
        group_delay = self.delay_median[:, None] * rng.lognormal(0, 0.4, (n, g))
        if LONG_DELAY_AREA is not None:
            group_delay[:, self.area_of_group == LONG_DELAY_AREA] *= 1.5
        self.group_delay = group_delay
        self.others_mean = np.exp(rng.uniform(math.log(20), math.log(20 * 60), n))
        self.gamma = rng.uniform(0, 1, n)
        crowd = (AREA_GROUPS.mean() / AREA_GROUPS)[self.area_of_group]  # per group
        self.others_t = self.others_mean[:, None] * crowd[None, :] ** self.gamma[:, None]
        self.independent = rng.random(n) < 0.5
        self.pos = self._layout(rng, n, a)
        d = np.linalg.norm(self.pos[:, :, None, :] - self.pos[:, None, :, :], axis=-1)
        self.travel_s = d / self.speed[:, None, None]
        self.init_seed = seed + 1

    @staticmethod
    def _layout(rng, n, a):
        pos = rng.uniform(0, MAP_M, (n, a, 2))
        for _ in range(50):  # push apart areas that are too close
            d = np.linalg.norm(pos[:, :, None] - pos[:, None, :], axis=-1)
            d[:, np.arange(a), np.arange(a)] = np.inf
            bad = (d < MIN_AREA_GAP_M).any(axis=2)
            if not bad.any():
                break
            pos[bad] = rng.uniform(0, MAP_M, (int(bad.sum()), 2))
        return pos

    def new_delay(self, rng, mask_shape_like):
        return self.group_delay[:, None, :] * rng.uniform(0.5, 1.5, mask_shape_like.shape)

    def initial_state(self):
        """Cubes up / respawn timers after hours of other players alone."""
        rng = np.random.default_rng(self.init_seed)
        shape = (self.n, CHANNELS, len(self.area_of_group))
        active = np.zeros(shape, bool)
        timer = rng.uniform(0, 1, shape) * self.group_delay[:, None, :]
        for _ in range(int(WARMUP_S / WARMUP_DT)):
            self.step_world(rng, active, timer, WARMUP_DT)
        return active, timer

    def step_world(self, rng, active, timer, dt):
        timer -= dt
        active |= timer <= 0
        p = -np.expm1(-dt / self.others_t)[:, None, :]
        taken = active & (rng.random(active.shape, dtype=np.float32) < p)
        active &= ~taken
        timer[taken] = self.new_delay(rng, active)[taken]
        np.maximum(timer, -1, out=timer)


def tour(world, areas):
    """Nearest-neighbour loop through `areas`, per session, from the densest."""
    n = world.n
    order = np.zeros((n, len(areas)), int)
    for s in range(n):
        left = list(areas[1:])
        cur = areas[0]
        seq = [cur]
        while left:
            nxt = min(left, key=lambda b: world.travel_s[s, cur, b])
            left.remove(nxt)
            seq.append(nxt)
            cur = nxt
        order[s] = seq
    return order


def run(world, plan, k, seed):
    rng = np.random.default_rng(seed)
    n, a = world.n, len(AREA_GROUPS)
    active, timer = world.initial_state()
    rows = np.arange(n)
    greedy = plan == "greedy"
    order = None if greedy else tour(world, plan)
    idx = np.zeros(n, int)
    area = np.zeros(n, int) if greedy else order[:, 0].copy()
    ch = np.zeros(n, int)
    ch_done = np.zeros(n, int)
    busy = np.zeros(n)  # travelling or changing channel
    sweep_left = world.sweep_s[rows, area].copy()
    last = np.full((n, k, a), -1e9)
    got = np.zeros(n, int)
    lost = np.zeros(n, int)
    group_area = world.area_of_group[None, :]
    t = 0.0
    while t < SESSION_S:
        world.step_world(rng, active, timer, DT)
        # Sweeping: cubes of this area on this channel turn up on the radar.
        sweeping = busy <= 0
        w = np.where(world.independent, ch, 0)
        here = active[rows, w] & (group_area == area[:, None]) & sweeping[:, None]
        seen = here & (rng.random(here.shape) < DT / world.sweep_s[rows, area][:, None])
        if seen.any():
            s_idx, g_idx = np.nonzero(seen)
            dist = rng.uniform(0, 100, len(s_idx))
            go = rng.uniform(2, 5, len(s_idx)) + dist / world.speed[s_idx] + 4.0
            win = rng.random(len(s_idx)) < np.exp(-go / world.others_t[s_idx, g_idx])
            np.add.at(got, s_idx[win], 1)
            np.add.at(lost, s_idx[~win], 1)
            np.add.at(sweep_left, s_idx, go)
            active[s_idx, w[s_idx], g_idx] = False
            timer[s_idx, w[s_idx], g_idx] = (
                world.group_delay[s_idx, g_idx] * rng.uniform(0.5, 1.5, len(s_idx))
            )
        busy -= DT
        sweep_left -= np.where(sweeping, DT, 0.0)
        done = sweeping & (sweep_left <= 0)
        if done.any():
            d = np.nonzero(done)[0]
            last[d, ch[d] % k, area[d]] = t
            if greedy:
                tsince = t - last[d]  # (m, k, a)
                value = AREA_GROUPS[None, None, :] * -np.expm1(-tsince / GREEDY_TAU_S)
                cost = (
                    world.travel_s[d, area[d]][:, None, :]
                    + world.switch_s[d][:, None, None]
                    * (np.arange(k)[None, :, None] != (ch[d] % k)[:, None, None])
                    + world.sweep_s[d][:, None, :]
                )
                best = np.argmax((value / cost).reshape(len(d), -1), axis=1)
                nch, nar = np.divmod(best, a)
                busy[d] = world.travel_s[d, area[d], nar] + world.switch_s[d] * (nch != ch[d] % k)
                ch[d], area[d] = nch, nar
            else:
                hop = ch_done[d] < k - 1
                h, m = d[hop], d[~hop]
                ch[h] = (ch[h] + 1) % k
                ch_done[h] += 1
                busy[h] = world.switch_s[h]
                idx[m] = (idx[m] + 1) % order.shape[1]
                nxt = order[m, idx[m]]
                busy[m] = world.travel_s[m, area[m], nxt]
                area[m] = nxt
                ch_done[m] = 0
            sweep_left[d] = world.sweep_s[d, area[d]]
        t += DT
    hours = SESSION_S / 3600
    return got / hours, lost / hours


def summarize(results, world):
    names = list(results)
    per_hour = np.stack([results[s][0] for s in names])  # (strategies, n)
    best = np.argmax(per_hour + 1e-9 * np.random.default_rng(0).random(per_hour.shape), axis=0)
    out = {"sims": world.n, "strategies": {}}
    for i, s in enumerate(names):
        v = per_hour[i]
        out["strategies"][s] = {
            "mean": float(v.mean()),
            "median": float(np.median(v)),
            "p10": float(np.percentile(v, 10)),
            "p90": float(np.percentile(v, 90)),
            "best_in": float((best == i).mean()),
            "share_of_best": float(np.mean(v / np.maximum(per_hour.max(axis=0), 1e-9))),
            "missed_per_hour": float(results[s][1].mean()),
        }
    pos = world.pos[:, :3]
    top3_loop = sum(np.linalg.norm(pos[:, i] - pos[:, (i + 1) % 3], axis=-1) for i in range(3))
    scen = {
        "channels independent": world.independent,
        "channels shared": ~world.independent,
        "little competition (cubes last >5 min)": world.others_mean > 5 * 60,
        "heavy competition (cubes last <1 min)": world.others_mean < 60,
        "fast respawn (median <15 min)": world.delay_median < 15 * 60,
        "slow respawn (median >45 min)": world.delay_median > 45 * 60,
        "dense areas crowded (gamma >0.7)": world.gamma > 0.7,
        "dense areas not crowded (gamma <0.3)": world.gamma < 0.3,
        "top 3 areas close together (<2.5 km loop)": top3_loop < 2500,
        "top 3 areas far apart (>4.5 km loop)": top3_loop > 4500,
    }
    out["scenarios"] = {}
    out["median_session"] = {
        "respawn_median_min": float(np.median(world.delay_median) / 60),
        "cube_lifetime_s": float(np.median(world.others_mean)),
    }
    for label, mask in scen.items():
        means = per_hour[:, mask].mean(axis=1)
        order = np.argsort(-means)
        out["scenarios"][label] = {
            "sims": int(mask.sum()),
            "ranking": [[names[j], float(means[j])] for j in order],
        }
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--zone", choices=sorted(ZONES), default="altgard")
    ap.add_argument("--sims", type=int, default=10000)
    ap.add_argument("--seed", type=int, default=20261017)
    ap.add_argument("--json", default="")
    args = ap.parse_args()
    names = use_zone(args.zone)
    print(f"{args.zone}: {AREA_GROUPS.sum()} groups in {len(AREA_GROUPS)} areas; densest: {names[0]}\n")
    world = World(args.sims, args.seed)
    results = {}
    for i, (name, (plan, k)) in enumerate(strategies().items()):
        t0 = time.time()
        results[name] = run(world, plan, k, args.seed + 100 + i)
        print(f"{name:40s} {results[name][0].mean():5.2f} cubes/h  ({time.time() - t0:.0f} s)", flush=True)
    out = summarize(results, world)
    print()
    print(f"{'rotation':40s} {'mean':>5s} {'median':>6s} {'p10':>5s} {'p90':>5s} {'best in':>8s} {'of best':>8s}")
    for s, r in sorted(out["strategies"].items(), key=lambda kv: -kv[1]["mean"]):
        print(
            f"{s:40s} {r['mean']:5.2f} {r['median']:6.2f} {r['p10']:5.2f} {r['p90']:5.2f}"
            f" {r['best_in']:8.1%} {r['share_of_best']:8.1%}"
        )
    for label, sc in out["scenarios"].items():
        top = ", ".join(f"{n} {v:.2f}" for n, v in sc["ranking"][:3])
        print(f"\n{label} ({sc['sims']} sims): {top}")
    if args.json:
        with open(args.json, "w") as f:
            json.dump(out, f, indent=1)


if __name__ == "__main__":
    main()
