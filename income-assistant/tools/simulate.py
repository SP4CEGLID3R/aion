"""Monte Carlo simulation: follow the app's plan every day and see where the kinah goes.

Usage:
  python tools/simulate.py HISTORY [--days 50] [--start 20m] [--hours 8] [--runs 300] [--out DIR]

HISTORY is either the app's own history.db (in %APPDATA%\\Aion2IncomeAssistant, filled by every
Refresh) or a JSON file:

HISTORY.json = {"snapshots": [{"ts": epoch, "rows": [market rows as /api/items returns]}, ...],
                "kaisinel_sales": {item_key: {"sold28": units}}}   (optional real sales volumes)

Each simulated day:
  1. morning (start of your session, peak hours): the app analyses a real market snapshot
     and makes its plan for your cash (Safe + Moderate only, sell within 1 day);
  2. you buy what it says and craft (success / Splendent rolled with the app's odds);
  3. selling, with the in-game limits: at most 10 listings up at once, ~5 minutes to put
     one up, buyers take whole listings. While you're online (the busiest ~55% of the
     day's trades) you win ONLINE_SHARE of an item's buyers and relist as lots sell;
     offline each listed lot can still sell once, at OFFLINE_SHARE;
  4. what sold is paid at the real price of the next day's snapshot, minus tax and fee;
     leftovers carry to the next day.
Days are drawn at random from the real history (a block bootstrap of real day-to-day moves).
"""
from __future__ import annotations

import argparse
import json
import math
import os
import random
import statistics
import sys
import tempfile
from dataclasses import dataclass

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

from aion_income.engine import Engine, MarketData, Settings  # noqa: E402
from aion_income.opportunities import LISTING_MINUTES, LISTING_SLOTS  # noqa: E402
from aion_income.opportunities import CALIBRATION  # noqa: E402
from aion_income.planner import MAX_SHARE, OVERHEAD_MINUTES, make_plan  # noqa: E402
from aion_income.recipes import _load_json, book_from_compact  # noqa: E402

PEAK_TRADE_SHARE = 0.55   # share of a day's trades in the busiest 8 hours
SERVER = {"server_id": 0, "name": "Lumiel", "faction": "Asmodian"}


def load_history(path: str) -> dict:
    """JSON history file, or the app's history.db (snapshots saved by every Refresh)."""
    if not path.endswith(".db"):
        return json.load(open(path, encoding="utf-8"))
    import sqlite3
    db = sqlite3.connect(path)
    meta = {}
    row = db.execute("SELECT body FROM market ORDER BY fetched DESC LIMIT 1").fetchone()
    for r in json.loads(row[0]) if row else []:
        meta[str(r.get("item_key"))] = (r.get("name"), r.get("category"))
    by_ts: dict = {}
    for item, ts, price, listings, sold in db.execute(
            "SELECT item, ts, price, listings, units_sold FROM snapshots ORDER BY ts"):
        bucket = round(ts / 600) * 600  # rows of one refresh share (almost) the same capture time
        name, cat = meta.get(item, (item, None))
        by_ts.setdefault(bucket, []).append({"item_key": int(item), "name": name, "category": cat or {},
                                             "price": price, "listings": listings, "units_sold": sold or 0})
    return {"snapshots": [{"ts": t, "rows": rows} for t, rows in sorted(by_ts.items())]}


def parse_kinah(t: str) -> float:
    t = t.lower().replace(",", "")
    mult = 1e6 if t.endswith("m") else 1e3 if t.endswith("k") else 1
    return float(t.rstrip("mk")) * mult


def poisson(rng, lam):
    if lam <= 0:
        return 0
    if lam > 50:
        return max(0, int(round(rng.gauss(lam, math.sqrt(lam)))))
    k, p, limit = 0, 1.0, math.exp(-lam)
    while True:
        p *= rng.random()
        if p <= limit:
            return k
        k += 1


@dataclass
class Strategy:
    """Which of the app's suggestions to follow."""
    name: str = "app default"
    risks: tuple = ("Safe", "Moderate")
    methods: tuple = ("Craft", "Resell")
    min_roi: float = 0.0
    max_roi: float = float("inf")
    min_market_per_day: float = 0.0
    exclude: tuple = ()          # skip suggestions with a warning containing any of these
    allow_risky_if_only: tuple = ()  # Risky allowed when every warning matches one of these
    calibration: tuple | None = CALIBRATION  # rank by realistic profit (A, B); None = raw prediction
    max_share: float = MAX_SHARE     # most of the budget one item may take
    batch_days: float = 3.0          # buy this many days of sales at once (the app's 3-day plan)
    sequential: bool = False         # replay real days in order from a random start (keeps real trends)
    weekly_timing: bool = False      # stock up Mon-Wed before the weekly reset, buy little Thu-Sun

    def accept(self, op) -> bool:
        if op.method not in self.methods or not (self.min_roi <= op.roi <= self.max_roi):
            return False
        if (op.market_per_day or 0) < self.min_market_per_day:
            return False
        if any(x in r for r in op.reasons for x in self.exclude):
            return False
        if op.risk in self.risks:
            return True
        if op.risk == "Risky" and self.allow_risky_if_only:
            return all(any(x in r for x in self.allow_risky_if_only) for r in op.reasons)
        return False


class Market:
    """Pre-computed app analyses of every usable real day."""

    def __init__(self, hist, settings, volume=1.0):
        snaps = sorted(hist["snapshots"], key=lambda s: s["ts"])
        sales = hist.get("kaisinel_sales") or {}
        server = hist.get("server") or SERVER
        self.server = server
        book = book_from_compact(_load_json("recipes.json"), "bundled")
        self.engine = Engine(tempfile.mkdtemp(), book=book)
        series = {}
        self.analyses = []
        for s in snaps:
            rows = []
            for r in s["rows"]:
                r = dict(r)
                k = str(r["item_key"])
                if k in sales:
                    r["units_sold"] = int(sales[k]["sold28"] * volume)
                elif r.get("units_sold"):
                    r["units_sold"] = int(r["units_sold"] * volume)
                rows.append(r)
                if r.get("price"):
                    series.setdefault(k, []).append((s["ts"], r["price"], r.get("listings") or 0))
            window = {k: list(v) for k, v in series.items()}  # everything seen so far, like the app's history
            data = MarketData(server=server, rows=rows, fetched_at=s["ts"], snapshots=window)
            self.analyses.append((s["ts"], self.engine.analyse(data, settings)))
        # a "day" = a real snapshot with enough history behind it and one ~24 h later
        self.days = []
        t0 = self.analyses[0][0]
        for i, (ts, _) in enumerate(self.analyses):
            if ts - t0 < 30 * 3600:
                continue
            nxt = [j for j, (t2, _) in enumerate(self.analyses) if 18 * 3600 <= t2 - ts <= 30 * 3600]
            if nxt:
                self.days.append((i, min(nxt, key=lambda j: abs(self.analyses[j][0] - ts - 86400))))
        if not self.days:
            raise SystemExit("not enough history: need snapshots spanning at least ~2 days")
        self._px = {}
        self._ops = {}

    def prices(self, i):
        """item -> (sell price, low-end price) for analysis i, computed once."""
        if i not in self._px:
            self._px[i] = {k: (st.sell_price or 0.0, st.sell_pessimistic or 0.0)
                           for k, st in self.analyses[i][1].stats.items()}
        return self._px[i]

    def ops_for(self, i, strategy):
        key = (i, id(strategy))
        if key not in self._ops:
            self._ops[key] = [o for o in self.analyses[i][1].ops if strategy.accept(o)]
        return self._ops[key]


def run_once(mkt: Market, rng, days, start, hours, online_share, offline_share, risks, keep,
             strategy: Strategy | None = None, trades: list | None = None):
    """One 50-day life. Returns (net worth per day, daily log); appends every trade to `trades`."""
    strategy = strategy or Strategy(risks=tuple(risks))
    cash = float(start)
    inv: dict[str, list] = {}        # item key -> FIFO lots [units, trade index]
    lot_of: dict[str, int] = {}
    rate_of: dict[str, float] = {}   # item key -> market units per day
    my_trades = []
    worth, log = [], []
    minutes_cap = hours * 60 - OVERHEAD_MINUTES

    totals: dict[str, float] = {}

    def held(k):
        return totals.get(k, 0.0)

    def add(k, units, tid):
        if units > 0:
            inv.setdefault(k, []).append([units, tid])
            totals[k] = totals.get(k, 0.0) + units

    def take(k, units, unit_price):
        lots = inv.get(k, [])
        while units > 1e-9 and lots:
            u = min(units, lots[0][0])
            totals[k] -= u
            my_trades[lots[0][1]]["revenue"] += u * unit_price
            my_trades[lots[0][1]]["sold"] += u
            lots[0][0] -= u
            units -= u
            if lots[0][0] <= 1e-9:
                lots.pop(0)

    seq_start = rng.randrange(len(mkt.days))
    for day in range(days):
        if strategy.sequential:
            i, j = mkt.days[(seq_start + day) % len(mkt.days)]
        else:
            i, j = rng.choice(mkt.days)
        a = mkt.analyses[i][1]
        ops = mkt.ops_for(i, strategy)
        px_now, px_next = mkt.prices(i), mkt.prices(j)
        batch = strategy.batch_days
        if strategy.weekly_timing:
            import datetime as _dt
            wd = _dt.datetime.fromtimestamp(mkt.analyses[i][0], _dt.timezone.utc).weekday()
            batch = 5.0 if wd in (0, 1, 2) else 1.0
        plan = make_plan(ops, cash, horizon=1.0, play_hours=hours, calibration=strategy.calibration,
                         max_share=strategy.max_share, batch_days=batch)
        craft_minutes = 0.0
        bought = 0.0
        for line in plan.lines:
            op = line.op
            key = op.stats.key
            units = line.units
            have = held(key)
            if have > 0:  # already holding stock of this item: buy less
                units = max(0, units - int(have / max(op.outputs_per_unit, 1e-9)))
            if op.cost_unit > cash:
                continue
            units = min(units, int(cash // op.cost_unit))
            if units <= 0:
                continue
            cost = units * op.cost_unit
            cash -= cost
            bought += cost
            tid = len(my_trades)
            my_trades.append({"day": day, "method": op.method, "risk": op.risk, "name": op.name, "key": key,
                              "section": op.stats.section, "reasons": list(op.reasons), "cost": cost,
                              "units": units, "pred_profit": op.profit_unit * units, "pred_roi": op.roi,
                              "pred_sell": op.sell_price, "market_per_day": op.market_per_day or 0.0,
                              "lot": op.lot, "revenue": 0.0, "sold": 0.0, "out": 0.0,
                              "volatility": op.stats.volatility, "trend_short": op.stats.trend_short,
                              "listings": op.stats.listings, "realized_price": None})
            lot_of[key] = op.lot
            rate_of[key] = op.market_per_day or 0.0
            if op.method == "Craft":
                c = op.craft
                craft_minutes += units * 15 / 60
                succ = sum(1 for _ in range(units) if rng.random() < c.success)
                combo = 0
                if c.combo_key:
                    p = c.recipe.combo_chance * c.combo_rate
                    combo = sum(1 for _ in range(succ) if rng.random() < p)
                    if combo:
                        cs = a.stats.get(c.combo_key)
                        add(c.combo_key, combo, tid)
                        lot_of[c.combo_key] = 1
                        rate_of[c.combo_key] = (cs.sales_per_day or 0.0) if cs else 0.0
                out = (succ - combo) * c.recipe.out_qty
                add(key, out, tid)
                my_trades[tid]["out"] = out + combo
                my_trades[tid]["success_rate"] = c.success
                my_trades[tid]["successes"] = succ
            else:
                add(key, units, tid)
                my_trades[tid]["out"] = units

        # selling: up to 10 items listed, most valuable flow first
        def price(k):
            v = px_next.get(k) or px_now.get(k)
            return v[0] if v else 0.0
        listed = sorted((k for k in inv if held(k) > 1e-9), key=lambda k: -price(k) * min(held(k), rate_of.get(k, 0)))
        listed = listed[:LISTING_SLOTS]
        relists_left = max(0, int((minutes_cap - craft_minutes) / LISTING_MINUTES) - len(listed))
        revenue = 0.0
        for k in listed:
            lot = max(1, lot_of.get(k, 1))
            stock = held(k)
            lam_on = rate_of.get(k, 0.0) * PEAK_TRADE_SHARE * online_share / lot
            n_on = min(poisson(rng, lam_on), relists_left + 1, math.ceil(stock / lot))
            relists_left -= max(0, n_on - 1)
            units = min(stock, n_on * lot)
            stock -= units
            # end of session: relist (if stock and time) and maybe sell once while offline
            if stock > 0 and (n_on == 0 or relists_left > 0):
                if n_on > 0:
                    relists_left -= 1
                lam_off = rate_of.get(k, 0.0) * (1 - PEAK_TRADE_SHARE) * offline_share / lot
                if rng.random() < 1 - math.exp(-lam_off):
                    units += min(stock, lot)
            if units > 0:
                pr = price(k) * keep
                take(k, units, pr)
                revenue += units * pr
        cash += revenue
        stock_value = 0.0
        for k, u in totals.items():
            if u > 1e-9:
                v = px_next.get(k)
                if v and v[1]:
                    stock_value += u * v[1] * keep
        worth.append(cash + stock_value)
        log.append({"bought": bought, "revenue": revenue, "lines": len(plan.lines), "cash": cash,
                    "stock": stock_value, "listed": len(listed), "ops": len(ops)})
    # value whatever is still unsold at the last day's low-end price
    last = mkt.analyses[mkt.days[-1][1]][1]
    for k, lots in inv.items():
        st = last.stats.get(k)
        val = (st.sell_pessimistic or 0.0) * keep if st else 0.0
        for u, tid in lots:
            my_trades[tid]["unsold_value"] = my_trades[tid].get("unsold_value", 0.0) + u * val
    for t in my_trades:
        t["pnl"] = t["revenue"] + t.get("unsold_value", 0.0) - t["cost"]
    if trades is not None:
        trades.extend(my_trades)
    return worth, log


def main(argv=None):
    ap = argparse.ArgumentParser()
    ap.add_argument("history")
    ap.add_argument("--days", type=int, default=50)
    ap.add_argument("--start", default="20m")
    ap.add_argument("--hours", type=float, default=8.0)
    ap.add_argument("--runs", type=int, default=300)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--out", default=None)
    ap.add_argument("--include-risky", action="store_true", help="also follow Risky suggestions")
    args = ap.parse_args(argv)
    hist = load_history(args.history)
    start = parse_kinah(args.start)

    scenarios = []
    for lv_name, lv in (("No crafting (resell only)", 0), ("Crafter, all professions lv 60", 60),
                        ("Crafter, all professions lv 100", 100)):
        for mk_name, on, off, vol in (("expected", 0.30, 0.10, 1.0), ("cautious", 0.15, 0.05, 0.5)):
            scenarios.append((lv_name, lv, mk_name, on, off, vol))

    results = []
    cache = {}
    for lv_name, lv, mk_name, on, off, vol in scenarios:
        s = Settings()
        s.levels = {p: lv for p in s.levels}
        s.play_hours = args.hours
        if args.include_risky:
            s.risks = ["Safe", "Moderate", "Risky"]
        ck = (lv, vol)
        if ck not in cache:
            cache[ck] = Market(hist, s, vol)
        mkt = cache[ck]
        rng = random.Random(args.seed)
        finals, curves, logs = [], [], []
        for _ in range(args.runs):
            w, log = run_once(mkt, rng, args.days, start, args.hours, on, off, s.risks, s.keep)
            finals.append(w[-1])
            curves.append(w)
            logs.append(log)
        finals.sort()
        q = lambda p: finals[min(len(finals) - 1, int(p * len(finals)))]  # noqa: E731
        med_curve = [statistics.median(c[d] for c in curves) for d in range(args.days)]
        p10_curve = [sorted(c[d] for c in curves)[int(0.1 * len(curves))] for d in range(args.days)]
        p90_curve = [sorted(c[d] for c in curves)[int(0.9 * len(curves))] for d in range(args.days)]
        avg_lines = statistics.fmean(l["lines"] for lg in logs for l in lg)
        avg_bought = statistics.fmean(l["bought"] for lg in logs for l in lg)
        res = {"scenario": lv_name, "market": mk_name, "p10": q(0.1), "median": q(0.5), "p90": q(0.9),
               "loss_runs": sum(1 for f in finals if f < start) / len(finals),
               "median_curve": med_curve, "p10_curve": p10_curve, "p90_curve": p90_curve,
               "avg_items_per_day": avg_lines, "avg_spent_per_day": avg_bought, "days_sampled": len(mkt.days)}
        results.append(res)
        print(f"{lv_name:34} {mk_name:9} final p10 {q(0.1)/1e6:7.1f}M  median {q(0.5)/1e6:7.1f}M  "
              f"p90 {q(0.9)/1e6:7.1f}M  (profit median {(q(0.5)-start)/1e6:+.1f}M, "
              f"{(q(0.5)-start)/args.days/1e6:+.2f}M/day, losing runs {res['loss_runs']:.0%}, "
              f"~{avg_lines:.1f} items/day, spends ~{avg_bought/1e6:.1f}M/day)", flush=True)
    if args.out:
        os.makedirs(args.out, exist_ok=True)
        with open(os.path.join(args.out, "simulation.json"), "w") as f:
            json.dump({"start": start, "days": args.days, "hours": args.hours, "runs": args.runs,
                       "results": results}, f, indent=1)
    return results


if __name__ == "__main__":
    main()
