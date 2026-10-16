"""Spend a budget across opportunities, and suggest how much kinah to hold.

Fast sellers return their kinah quickly and it can be spent again, so each
opportunity is valued by what it earns over the whole horizon per kinah it ties
up: you buy about one day of what you can sell, sell it, re-buy, repeat.
"""
from __future__ import annotations

import math
from dataclasses import dataclass

from .opportunities import CALIBRATION, LISTING_SLOTS, Opportunity, realistic_roi

# Defaults learned from simulated 50-day runs on EU and NA servers (see tools/simulate.py); both
# regions agreed on these:
MAX_SHARE = 0.15     # at most 15% of the budget in one item: 20-25% grew a little faster on average
                     # but had worse bad-luck results on Kaisinel (EU, real sales) and on NA
MAX_STOCK_DAYS = 10  # buy up to 10 days of sales when a good trade is there: good trades don't show up
                     # every day but you can sell every day, and the 10 listing slots stay full
                     # (EU: 2 days -> 52M, 7 -> 78M, 10 -> 81M; NA: 7 -> 102M, 10 -> 107M) ...

# ... but only while prices hold. Stock bought now sells at later prices, so in a falling market
# long storage loses: in EU runs with prices falling 3%/day, 10 days of stock ended at 37.5M
# (27% of runs losing money) vs 42.1M with 3 days; at -1.5%/day 5, 7 and 10 days tied (~53M);
# at 0% or rising, 10 days won. So the stock length follows how the plan's items moved this week.
STOCK_BY_TREND = ((-0.005, 10.0), (-0.02, 5.0), (float("-inf"), 3.0))  # (daily trend at least, days)
STOCK_UNKNOWN = 5.0  # no trend known yet: the choice that was never far from best


def stock_days_for(trend_per_day: float | None) -> float:
    """Days of sales to buy at once, given how prices are moving (e.g. -0.01 = falling 1%/day)."""
    if trend_per_day is None:
        return STOCK_UNKNOWN
    return next(days for floor, days in STOCK_BY_TREND if trend_per_day >= floor)


def items_trend(ops, market_drift: float | None = None) -> float | None:
    """Typical daily price move of the items these opportunities sell, weighted by money at stake;
    falls back to the market-wide day-over-day drift."""
    pts = []
    for op in ops:
        st = op.stats
        if st is not None and st.trend_7d is not None and st.trend_7d > -0.95:
            pts.append(((1 + st.trend_7d) ** (1 / 7) - 1, max(op.capital, 1.0)))
    if pts:
        pts.sort()
        half, acc = sum(w for _, w in pts) / 2, 0.0
        for t, w in pts:
            acc += w
            if acc >= half:
                return t
    if market_drift is not None and market_drift != 1.0:
        return market_drift - 1
    return None

OVERHEAD_MINUTES = 20  # refreshing, walking to the market, buying ingredients

HOLD_CAP = 30_000_000  # holding more than this is unrealistic for most players


@dataclass
class PlanLine:
    op: Opportunity
    units: int          # buy / craft this many now
    capital: float      # kinah tied up now
    profit: float       # expected profit over the horizon (re-buying as it sells)
    sold_over_horizon: float
    minutes: float = 0.0  # your time per day for this line


@dataclass
class Plan:
    lines: list
    budget: float
    horizon: float
    capital: float
    profit: float
    minutes: float = 0.0     # your time per day (listing + crafting)
    play_minutes: float = 0.0

    @property
    def profit_per_day(self) -> float:
        return self.profit / self.horizon if self.horizon else 0.0

    @property
    def leftover(self) -> float:
        return self.budget - self.capital


def _terms(op: Opportunity, horizon: float, calibration=None, batch_days: float = 1.0):
    """(units held at once, capital, profit over horizon) for running op at full speed.
    With a calibration, profit is what such trades really returned, not the raw prediction;
    a calibration can be (A, B) or a function op -> realistic ROI."""
    if calibration is None:
        profit_unit = op.profit_unit
    elif callable(calibration):
        profit_unit = op.cost_unit * calibration(op)
    else:
        profit_unit = op.cost_unit * realistic_roi(op.roi, calibration)
    if op.your_per_day <= 0 or op.cost_unit <= 0 or profit_unit <= 0:
        return None
    units = max(1, math.ceil(op.your_per_day * min(horizon, 1.0) * batch_days))
    sold = op.your_per_day * horizon
    return units, units * op.cost_unit, profit_unit * sold, sold


def _greedy(cands, budget, horizon, minutes_cap, limit, max_share=1.0):
    lines, left, minutes_left = [], budget, minutes_cap
    line_cap = budget * max_share
    used_keys = set()
    for op, units, capital, profit, sold in cands:
        if left < op.cost_unit or minutes_left <= 0:
            continue
        sell_key = op.stats.key if op.stats else (op.method, op.key)
        if sell_key in used_keys:
            continue  # buyers of one item are shared: don't count them twice
        frac = 1.0
        if op.minutes_per_day > minutes_left:
            frac = minutes_left / op.minutes_per_day  # not enough time to list all of it
        take = min(int(math.floor(units * frac)) or 1, units, int(left // op.cost_unit),
                   int(line_cap // op.cost_unit))  # don't put too much into one item
        if take <= 0:
            continue
        frac = take / units
        lines.append(PlanLine(op, take, take * op.cost_unit, profit * frac, sold * frac, op.minutes_per_day * frac))
        used_keys.add(sell_key)
        left -= take * op.cost_unit
        minutes_left -= op.minutes_per_day * frac
        if len(lines) >= min(limit, LISTING_SLOTS):
            break  # one listing slot per item at least
    return lines


def make_plan(ops: list[Opportunity], budget: float, horizon: float = 3.0, limit: int = 25,
              play_hours: float = 8.0, calibration=CALIBRATION, max_share: float = MAX_SHARE,
              batch_days: float | None = None) -> Plan:
    """Best use of a budget within the market's limits: at most 10 items listed at once and
    5 minutes per listing, so your playing time caps how much you can sell per day."""
    minutes_cap = max(0.0, play_hours * 60 - OVERHEAD_MINUTES)
    if batch_days is None:
        batch_days = min(max(horizon, 1.0), MAX_STOCK_DAYS)
    cands = []
    for op in ops:
        t = _terms(op, horizon, calibration, batch_days)
        if t and op.minutes_per_day > 0:
            cands.append((op,) + t)
    best = []
    # capital-bound and time-bound orderings; keep whichever earns more
    for key in (lambda c: c[3] / c[2], lambda c: c[3] / c[0].minutes_per_day):
        lines = _greedy(sorted(cands, key=key, reverse=True), budget, horizon, minutes_cap, limit, max_share)
        if sum(l.profit for l in lines) > sum(l.profit for l in best):
            best = lines
    return Plan(best, budget, horizon, sum(l.capital for l in best), sum(l.profit for l in best),
                sum(l.minutes for l in best), play_hours * 60)


def make_adaptive_plan(ops: list[Opportunity], budget: float, horizon: float = 10.0, play_hours: float = 8.0,
                       market_drift: float | None = None):
    """make_plan with the stock length set by how the chosen items' prices are moving.
    Returns (plan, stock days, daily trend or None)."""
    first = make_plan(ops, budget, horizon, play_hours=play_hours)
    trend = items_trend([l.op for l in first.lines], market_drift)
    days = min(max(horizon, 1.0), stock_days_for(trend), MAX_STOCK_DAYS)
    if days >= min(max(horizon, 1.0), MAX_STOCK_DAYS):
        return first, days, trend
    return make_plan(ops, budget, horizon, play_hours=play_hours, batch_days=days), days, trend


@dataclass
class HoldTier:
    label: str
    amount: float
    profit_per_day: float


GRID = [250_000, 500_000, 750_000, 1e6, 1.5e6, 2e6, 3e6, 4e6, 5e6, 7.5e6, 10e6, 12.5e6, 15e6, 20e6, 25e6, 30e6]
TIERS = (("Low", 0.5, 5e6), ("Mid", 0.8, 15e6), ("High", 0.95, HOLD_CAP))


def holding_tiers(ops: list[Opportunity], horizon: float = 3.0, play_hours: float = 8.0,
                  batch_days: float | None = None) -> list[HoldTier]:
    """Low / mid / high amounts worth holding, from where extra kinah stops paying.

    Each tier is the smallest amount that earns its share (50% / 80% / 95%) of what
    30 million could earn, but never more than 5M / 15M / 30M: holding more than
    that is unrealistic for most players.
    """
    curve = [(b, make_plan(ops, b, horizon, play_hours=play_hours, batch_days=batch_days).profit_per_day) for b in GRID]
    best = curve[-1][1]
    if best <= 0:
        return []
    tiers = []
    prev = 0.0
    for label, share, cap in TIERS:
        choice = None
        for b, p in curve:
            if b <= prev:
                continue
            if p >= best * share or b >= cap:
                choice = (b, p)
                break
        if choice is None:
            break
        tiers.append(HoldTier(label, choice[0], choice[1]))
        prev = choice[0]
    return tiers
