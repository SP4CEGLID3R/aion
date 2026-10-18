#!/usr/bin/env python3
"""Monte Carlo comparison of hidden-cube routes on a real map (AION 2).

Uses a zone's mapdata.json from aion2maps.com (not included; download it
yourself, e.g. maps/World_D_A/mapdata.json for Altgard): the real cube
spawners ("groups") and their candidate spots. One server, no channels.

* Each group has at most one cube, on one of its spots (chosen at random
  each time it spawns).
* You fly a route at mount speed; the radar shows any cube within 100 m
  (cubes are sent from about 100 m away). A route visits every spot of its
  groups: spots within 70 m of each other share a waypoint, waypoints are
  joined into a short loop.
* You then go and open the cube (reaction 2-5 s + distance / speed + 4 s to
  open) and get it only if nobody else opens it first; then back to the
  route.
* Unknowns are drawn per session, as in cube_rotation_sim.py: respawn delay
  (median log-uniform 5-90 min), how long an available cube survives other
  players (20 s - 20 min), how much more crowded Dredgion Crash Site is (the
  area guides recommend; 1-3x), speed (8-14 m/s, routes 1.2x longer than a
  straight line).
* "Smart" rotations fly one area's loop, then go to the area whose cubes
  should most be back (groups x time away, guessed respawn 20 min) per
  second of travel plus loop.

    python3 tools/cube_route_sim.py mapdata.json --sims 10000
    python3 tools/cube_route_sim.py mapdata.json --sims 100000 --competition high \
        --focused --workers 4

--competition high: fresh cubes survive other players only 20 s - 2 min.
--focused: the AFK spots, the best areas and the optimised loops only.
"""

import argparse
import json
import math
import re
import time
from multiprocessing import Pool

import numpy as np

RADAR_M = 100.0
COVER_M = 70.0
DETOUR = 1.2
SESSION_S = 2 * 3600
DT = 5.0
WARMUP_S = 6 * 3600
WARMUP_DT = 30.0
SMART_TAU_S = 20 * 60
FAMOUS = "Dredgion Crash Site"


def region_of(spawner):
    if "Dredgion" in spawner or "CalderonHill" in spawner:
        return FAMOUS
    name = re.sub(r"^EnvObj_[A-Z]\d_", "", spawner)
    name = re.sub(r"_(UnderWater_)?HiddenBox.*$", "", name)
    return re.sub(r"(?<=[a-z])(?=[A-Z])", " ", name).replace("_", " ")


def waypoints(spots):
    """Points that pass within COVER_M of every spot."""
    left = np.arange(len(spots))
    out = []
    while len(left):
        p = spots[left]
        near = np.linalg.norm(p[:, None] - p[None], axis=-1) < COVER_M
        k = int(np.argmax(near.sum(1)))
        out.append(p[near[k]].mean(0))
        left = left[~near[k]]
    return np.array(out)


def short_loop(w):
    """Nearest-neighbour loop improved by 2-opt."""
    n = len(w)
    if n < 4:
        return w
    order = [0]
    left = set(range(1, n))
    while left:
        j = min(left, key=lambda b: np.linalg.norm(w[order[-1]] - w[b]))
        order.append(j)
        left.remove(j)
    order = np.array(order)
    d = np.linalg.norm(w[:, None] - w[None], axis=-1)
    improved = True
    while improved:
        improved = False
        for i in range(1, n - 1):
            a, b = order[i - 1], order[i]
            c, e = order[i:], np.roll(order, -1)[i:]
            gain = d[a, b] + d[c, e] - d[a, c] - d[b, e]
            k = int(np.argmax(gain))
            if gain[k] > 1e-6:
                order[i : i + k + 1] = order[i : i + k + 1][::-1]
                improved = True
    return w[order]


class Route:
    """A closed loop of waypoints with arclength lookup."""

    def __init__(self, pts):
        self.pts = pts
        seg = np.linalg.norm(np.roll(pts, -1, axis=0) - pts, axis=1)
        self.cum = np.concatenate([[0.0], np.cumsum(seg)])
        self.length = max(float(self.cum[-1]), 1.0)

    def at(self, s):
        s = np.mod(s, self.length)
        i = np.clip(np.searchsorted(self.cum, s, side="right") - 1, 0, len(self.pts) - 1)
        a, b = self.pts[i], self.pts[(i + 1) % len(self.pts)]
        seg = np.maximum(self.cum[i + 1] - self.cum[i], 1e-9)
        return a + (b - a) * ((s - self.cum[i]) / seg)[:, None]


def build_route(regions_spots):
    """One loop through several areas: each area's own loop, chained."""
    loops = [short_loop(waypoints(s)) for s in regions_spots]
    if len(loops) == 1:
        return Route(loops[0])
    # Chain the areas nearest-first, entering each at its nearest waypoint.
    out = [loops[0]]
    left = loops[1:]
    while left:
        last = out[-1][-1]
        j = min(range(len(left)), key=lambda k: np.linalg.norm(left[k] - last, axis=1).min())
        lp = left.pop(j)
        start = int(np.argmin(np.linalg.norm(lp - last, axis=1)))
        out.append(np.roll(lp, -start, axis=0))
    return Route(np.vstack(out))


def seg_dist(points, route):
    """Distance from each point to the nearest segment of the loop."""
    a = route.pts
    b = np.roll(a, -1, axis=0)
    ab = b - a
    t = np.einsum("psk,sk->ps", points[:, None, :] - a[None], ab) / np.maximum((ab**2).sum(1), 1e-9)[None]
    proj = a[None] + np.clip(t, 0, 1)[..., None] * ab[None]
    return np.linalg.norm(points[:, None, :] - proj, axis=2).min(1)


def coverage(m, route):
    """Expected groups a lap passes within radar range of: per group, the
    share of its spots near the route."""
    near = seg_dist(m.all_spots, route) < RADAR_M
    return float(np.sum(near * m.spot_weight))


def best_loop(m, budget_m):
    """A loop of at most `budget_m` that passes as many cubes as it can:
    prize-collecting insertion, each spot worth 1 / spots of its group."""
    w = waypoints(m.all_spots)
    cov = np.linalg.norm(w[:, None] - m.all_spots[None], axis=2) < COVER_M  # (W, spots)
    covered = np.zeros(len(m.all_spots), bool)
    gain0 = cov @ m.spot_weight
    # Start where the most cube value lies within 300 m.
    dense = (np.linalg.norm(w[:, None] - w[None], axis=2) < 300) @ gain0
    loop = [int(np.argmax(dense))]
    covered |= cov[loop[0]]
    length = 0.0
    while True:
        pts = w[loop]
        nxt = np.roll(pts, -1, axis=0)
        d_in = np.linalg.norm(w[:, None] - pts[None], axis=2)
        d_out = np.linalg.norm(w[:, None] - nxt[None], axis=2)
        base = np.linalg.norm(nxt - pts, axis=1)
        extra = d_in + d_out - base[None]  # (W, edges)
        e = np.argmin(extra, axis=1)
        delta = extra[np.arange(len(w)), e]
        gain = (cov & ~covered[None]) @ m.spot_weight
        score = gain / np.maximum(delta, 30.0)
        score[loop] = -1
        score[length + delta > budget_m] = -1
        k = int(np.argmax(score))
        if score[k] <= 0:
            break
        loop.insert(int(e[k]) + 1, k)
        covered |= cov[k]
        length += delta[k]
    return Route(short_loop(w[loop]))


class Map:
    def __init__(self, path):
        d = json.load(open(path))
        self.name = d.get("map", "?")
        self.group_region = []
        self.spots = []
        for c in d["cubes"]:
            self.group_region.append(region_of(c["s"]))
            self.spots.append(np.array([[p[0], p[2]] for p in c["pos"]], float))
        self.regions = sorted(set(self.group_region))
        self.n_spots = np.array([len(s) for s in self.spots])
        smax = self.n_spots.max()
        self.spot_xy = np.zeros((len(self.spots), smax, 2))
        for g, s in enumerate(self.spots):
            self.spot_xy[g, : len(s)] = s
        self.all_spots = np.vstack(self.spots)
        self.spot_weight = np.concatenate([np.full(len(s), 1.0 / len(s)) for s in self.spots])

    def spots_of(self, region):
        return np.vstack([s for s, r in zip(self.spots, self.group_region) if r == region])

    def groups_in(self, region):
        return sum(r == region for r in self.group_region)


COMPETITION = {"any": (20, 20 * 60), "high": (20, 120)}


class World:
    def __init__(self, m, n, seed, competition="any"):
        rng = np.random.default_rng(seed)
        g = len(m.spots)
        self.m, self.n = m, n
        self.speed = rng.uniform(8, 14, n) / DETOUR
        self.delay_median = np.exp(rng.uniform(math.log(5 * 60), math.log(90 * 60), n))
        self.group_delay = self.delay_median[:, None] * rng.lognormal(0, 0.4, (n, g))
        lo, hi = COMPETITION[competition]
        self.others_mean = np.exp(rng.uniform(math.log(lo), math.log(hi), n))
        self.famous_crowd = rng.uniform(1, 3, n)
        famous = np.array([r == FAMOUS for r in m.group_region])
        self.others_t = self.others_mean[:, None] / np.where(famous[None, :], self.famous_crowd[:, None], 1.0)
        self.init_seed = seed + 1

    def respawn_spot(self, rng, shape_groups):
        k = (rng.random(shape_groups.shape) * self.m.n_spots[shape_groups]).astype(int)
        return self.m.spot_xy[shape_groups, k]

    def initial_state(self):
        rng = np.random.default_rng(self.init_seed)
        n, g = self.n, len(self.m.spots)
        groups = np.broadcast_to(np.arange(g), (n, g))
        active = np.zeros((n, g), bool)
        timer = rng.uniform(0, 1, (n, g)) * self.group_delay
        pos = self.respawn_spot(rng, groups)
        for _ in range(int(WARMUP_S / WARMUP_DT)):
            self.step(rng, active, timer, pos, WARMUP_DT)
        return active, timer, pos

    def step(self, rng, active, timer, pos, dt):
        timer -= dt
        spawn = (~active) & (timer <= 0)
        if spawn.any():
            s, g = np.nonzero(spawn)
            pos[s, g] = self.respawn_spot(rng, g)
            active |= spawn
        p = -np.expm1(-dt / self.others_t)
        taken = active & (rng.random(active.shape) < p)
        if taken.any():
            active &= ~taken
            timer[taken] = (self.group_delay * rng.uniform(0.5, 1.5, active.shape))[taken]


def run_fixed(world, route, seed):
    """Fly one loop again and again."""
    rng = np.random.default_rng(seed)
    n = world.n
    active, timer, pos = world.initial_state()
    s = rng.uniform(0, route.length, n)
    pause = np.zeros(n)
    got = np.zeros(n)
    t = 0.0
    while t < SESSION_S:
        world.step(rng, active, timer, pos, DT)
        me = route.at(s)
        flying = pause <= 0
        _collect(world, rng, active, timer, pos, me, flying, pause, got)
        s = s + np.where(flying, world.speed * DT, 0.0)
        pause -= DT
        t += DT
    return got / (SESSION_S / 3600)


def run_smart(world, region_names, routes, seed):
    """Fly an area's loop, then go to the most promising area."""
    rng = np.random.default_rng(seed)
    n, r_n = world.n, len(routes)
    active, timer, pos = world.initial_state()
    groups = np.array([world.m.groups_in(r) for r in region_names], float)
    loop_len = np.array([r.length for r in routes])
    entry = [r.pts for r in routes]
    cur = np.full(n, int(np.argmax(groups)))
    s0 = np.zeros(n)  # loop start (arclength)
    done = np.zeros(n)  # metres flown on the current loop
    trav_a = np.zeros((n, 2))
    trav_b = np.zeros((n, 2))
    trav_left = np.zeros(n)  # metres of travel to the next area
    trav_len = np.ones(n)
    last = np.full((n, r_n), -1e9)
    pause = np.zeros(n)
    got = np.zeros(n)
    t = 0.0
    while t < SESSION_S:
        world.step(rng, active, timer, pos, DT)
        me = np.zeros((n, 2))
        travelling = trav_left > 0
        f = 1 - trav_left / trav_len
        me[travelling] = trav_a[travelling] + (trav_b - trav_a)[travelling] * f[travelling, None]
        for r in range(r_n):
            on = (~travelling) & (cur == r)
            if on.any():
                me[on] = routes[r].at(s0[on] + done[on])
        flying = pause <= 0
        _collect(world, rng, active, timer, pos, me, flying, pause, got)
        step = np.where(flying, world.speed * DT, 0.0)
        trav_left = np.where(travelling, trav_left - step, trav_left)
        done = np.where(~travelling, done + step, done)
        finished = (~travelling) & (done >= loop_len[cur])
        if finished.any():
            d = np.nonzero(finished)[0]
            last[d, cur[d]] = t
            here = me[d]
            near = np.stack([np.linalg.norm(entry[r][None] - here[:, None], axis=2).min(1) for r in range(r_n)], 1)
            value = groups[None, :] * -np.expm1(-(t - last[d]) / SMART_TAU_S)
            cost = near / world.speed[d, None] + loop_len[None, :] / world.speed[d, None]
            nxt = np.argmax(value / cost, axis=1)
            for i, r in zip(d, nxt):
                k = int(np.argmin(np.linalg.norm(entry[r] - me[i], axis=1)))
                trav_a[i], trav_b[i] = me[i], entry[r][k]
                trav_len[i] = trav_left[i] = max(np.linalg.norm(entry[r][k] - me[i]), 1e-3)
                s0[i] = routes[r].cum[k]
            cur[d] = nxt
            done[d] = 0
        pause -= DT
        t += DT
    return got / (SESSION_S / 3600)


def _collect(world, rng, active, timer, pos, me, flying, pause, got):
    dist = np.linalg.norm(pos - me[:, None, :], axis=2)
    seen = active & (dist < RADAR_M) & flying[:, None]
    if not seen.any():
        return
    s, g = np.nonzero(seen)
    v = world.speed[s] * DETOUR
    go = rng.uniform(2, 5, len(s)) + dist[s, g] / v + 4.0
    win = rng.random(len(s)) < np.exp(-go / world.others_t[s, g])
    np.add.at(got, s[win], 1)
    np.add.at(pause, s, go + dist[s, g] / v)
    active[s, g] = False
    timer[s, g] = world.group_delay[s, g] * rng.uniform(0.5, 1.5, len(s))


def afk_spots(m, k=6, apart=300.0):
    """Standing points with the most cube value within radar range."""
    p = m.all_spots
    xs = np.arange(p[:, 0].min() - RADAR_M, p[:, 0].max() + RADAR_M, 10.0)
    zs = np.arange(p[:, 1].min() - RADAR_M, p[:, 1].max() + RADAR_M, 10.0)
    grid = np.array(np.meshgrid(xs, zs)).reshape(2, -1).T
    val = np.zeros(len(grid))
    for i in range(0, len(grid), 4000):
        near = np.linalg.norm(grid[i : i + 4000, None] - p[None], axis=2) < RADAR_M
        val[i : i + 4000] = near @ m.spot_weight
    picked = []
    for j in np.argsort(-val):
        if all(np.linalg.norm(grid[j] - grid[q]) > apart for q in picked):
            picked.append(j)
        if len(picked) == k:
            break
    return [(grid[j], float(val[j])) for j in picked]


def focused(m):
    c = {}
    for i, (pt, _) in enumerate(afk_spots(m)):
        c[f"AFK spot #{i + 1}"] = ("afk", pt)
    for r in ("West Gribade Canyon", "Gribade Canyon", "Fafnite Deposit", "Fierce Fight Hill",
              "Unnamed Cemetery", "Tranein Hill Land", FAMOUS):
        if r in m.regions:
            c[f"Camp {r}"] = [r]
    c["Gribade: West Gribade + Gribade + Fafnite"] = [
        r for r in ("West Gribade Canyon", "Gribade Canyon", "Fafnite Deposit") if r in m.regions]
    for km in (0.6, 0.8, 1, 1.2, 1.5, 2, 3, 4, 6):
        c[f"Optimised loop <= {km:g} km"] = ("best", km * 1000.0)
    return c


def candidates(m):
    """name -> list of areas (fixed loop) or ('smart', areas)."""
    regs = m.regions
    have = lambda *xs: [x for x in xs if x in regs]
    c = {}
    by_groups = sorted(regs, key=lambda r: -m.groups_in(r))
    for r in by_groups:
        c[f"Camp {r}"] = [r]
    c["Gribade: West Gribade + Gribade + Fafnite"] = have("West Gribade Canyon", "Gribade Canyon", "Fafnite Deposit")
    c["Gribade + East Gribade"] = have("West Gribade Canyon", "Gribade Canyon", "Fafnite Deposit", "East Gribade Canyon")
    c["South: Fierce Fight Hill + Purifying Forest"] = have("Fierce Fight Hill", "Purificatoryforest")
    c["South + Dranactus"] = have("Fierce Fight Hill", "Purificatoryforest", "Dranactus")
    c["West: Dredgion + Nameless Cemetery"] = have(FAMOUS, "Unnamed Cemetery")
    c["West + Tranein + Silent Hill"] = have(FAMOUS, "Unnamed Cemetery", "Tranein Hill Land", "Silence Hill")
    c["Whole map loop"] = list(regs)
    for km in (2, 3, 4, 6, 8, 12):
        c[f"Optimised loop <= {km} km"] = ("best", km * 1000.0)
    c["Smart: all areas"] = ("smart", list(regs))
    c["Smart: Gribade + South areas"] = (
        "smart",
        have("West Gribade Canyon", "Gribade Canyon", "Fafnite Deposit", "East Gribade Canyon",
             "Fierce Fight Hill", "Purificatoryforest", "Seggurheim"),
    )
    return {k: v for k, v in c.items() if (v[1] if isinstance(v, tuple) else v)}


def describe(m, route):
    """Areas a loop passes through, by share of their cube value near it."""
    near = seg_dist(m.all_spots, route) < RADAR_M
    region = np.concatenate([[r] * len(s) for s, r in zip(m.spots, m.group_region)])
    out = {}
    for r in m.regions:
        v = float(np.sum((near * m.spot_weight)[region == r]))
        if v >= 0.25:
            out[r] = round(v, 2)
    return dict(sorted(out.items(), key=lambda kv: -kv[1]))


def build(m, spec):
    """The fixed route of a candidate, or None for a smart rotation."""
    if isinstance(spec, tuple) and spec[0] == "afk":
        return Route(np.array([spec[1], spec[1] + 0.01]))
    if isinstance(spec, tuple) and spec[0] == "best":
        return best_loop(m, spec[1])
    if isinstance(spec, tuple):
        return None
    return build_route([m.spots_of(r) for r in spec])


def simulate_chunk(job):
    path, cands, n, seed, competition = job
    m = Map(path)
    world = World(m, n, seed, competition)
    region_route = {r: build_route([m.spots_of(r)]) for r in m.regions}
    out = {}
    for i, (name, spec) in enumerate(cands.items()):
        route = build(m, spec)
        if route is None:
            regs = spec[1]
            out[name] = run_smart(world, regs, [region_route[r] for r in regs], seed + 100 + i)
        else:
            out[name] = run_fixed(world, route, seed + 100 + i)
    return out, world.others_mean, world.delay_median, world.famous_crowd


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mapdata")
    ap.add_argument("--sims", type=int, default=10000)
    ap.add_argument("--seed", type=int, default=20261018)
    ap.add_argument("--competition", choices=sorted(COMPETITION), default="any")
    ap.add_argument("--focused", action="store_true")
    ap.add_argument("--workers", type=int, default=1)
    ap.add_argument("--chunk", type=int, default=5000)
    ap.add_argument("--only", default="", help="comma-separated candidate names")
    ap.add_argument("--json", default="")
    args = ap.parse_args()
    m = Map(args.mapdata)
    print(f"{m.name}: {len(m.spots)} groups, {m.n_spots.sum()} spots, {len(m.regions)} areas; "
          f"competition {args.competition}; {args.sims} sessions\n", flush=True)
    cands = focused(m) if args.focused else candidates(m)
    if args.only:
        keep = [x.strip() for x in args.only.split(",")]
        cands = {k: v for k, v in cands.items() if k in keep}
    info = {}
    for name, spec in cands.items():
        route = build(m, spec)
        if route is None:
            info[name] = {"groups": sum(m.groups_in(r) for r in spec[1]), "loop_m": None}
        else:
            info[name] = {"groups": coverage(m, route), "loop_m": route.length,
                          "areas": describe(m, route), "start": [round(float(v), 1) for v in route.pts[0]]}
    jobs = []
    left, k = args.sims, 0
    while left > 0:
        n = min(args.chunk, left)
        jobs.append((args.mapdata, cands, n, args.seed + 1000 * k, args.competition))
        left -= n
        k += 1
    t0 = time.time()
    parts = []
    with Pool(max(1, args.workers)) as pool:
        for j, part in enumerate(pool.imap(simulate_chunk, jobs)):
            parts.append(part)
            print(f"chunk {j + 1}/{len(jobs)} done ({time.time() - t0:.0f} s)", flush=True)
    names = list(cands)
    results = {k: np.concatenate([p[0][k] for p in parts]) for k in names}
    others = np.concatenate([p[1] for p in parts])
    delay = np.concatenate([p[2] for p in parts])
    crowd = np.concatenate([p[3] for p in parts])
    per_hour = np.stack([results[k] for k in names])
    best = np.argmax(per_hour + 1e-9 * np.random.default_rng(0).random(per_hour.shape), axis=0)
    top = np.maximum(per_hour.max(axis=0), 1e-9)
    print(f"\n{'route':44s} {'groups':>6s} {'loop':>7s} {'mean':>6s} {'±95%':>5s} {'p10':>5s} {'p90':>5s} {'best in':>8s} {'of best':>8s}")
    order = np.argsort(-per_hour.mean(axis=1))
    out = {"map": m.name, "sims": int(per_hour.shape[1]), "competition": args.competition, "routes": {}, "scenarios": {}}
    for j in order:
        v = per_hour[j]
        r = {"mean": float(v.mean()), "ci95": float(1.96 * v.std() / math.sqrt(len(v))),
             "p10": float(np.percentile(v, 10)), "p90": float(np.percentile(v, 90)),
             "best_in": float((best == j).mean()), "share_of_best": float(np.mean(v / top)), **info[names[j]]}
        out["routes"][names[j]] = r
        lm = r["loop_m"]
        loop = f"{lm / 1000:5.2f}km" if lm and lm > 1 else ("  stand" if lm else "      -")
        print(f"{names[j]:44s} {r['groups']:6.2f} {loop:>7s} {r['mean']:6.2f} {r['ci95']:5.2f} {r['p10']:5.2f} {r['p90']:5.2f}"
              f" {r['best_in']:8.1%} {r['share_of_best']:8.1%}")
    lo, hi = COMPETITION[args.competition]
    mid = math.sqrt(lo * hi)
    scen = {
        f"harder competition (cubes last <{mid:.0f} s)": others < mid,
        f"softer competition (cubes last >{mid:.0f} s)": others >= mid,
        "fast respawn (<15 min)": delay < 900,
        "medium respawn (15-45 min)": (delay >= 900) & (delay <= 2700),
        "slow respawn (>45 min)": delay > 2700,
        "Dredgion crowded (>2x)": crowd > 2,
    }
    for label, mask in scen.items():
        means = per_hour[:, mask].mean(axis=1)
        wins = np.bincount(best[mask], minlength=len(names)) / max(mask.sum(), 1)
        o = np.argsort(-means)
        out["scenarios"][label] = [[names[j], float(means[j]), float(wins[j])] for j in o]
        print(f"\n{label} ({int(mask.sum())}): " + ", ".join(f"{names[j]} {means[j]:.2f} ({wins[j]:.0%})" for j in o[:4]))
    if args.json:
        with open(args.json, "w") as f:
            json.dump(out, f, indent=1)


if __name__ == "__main__":
    main()
