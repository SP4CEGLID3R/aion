"""Spend a budget across opportunities, and suggest how much kinah to hold.

Fast sellers return their kinah quickly and it can be spent again, so each
opportunity is valued by what it earns over the whole horizon per kinah it ties
up: you buy about one day of what you can sell, sell it, re-buy, repeat.
"""
from __future__ import annotations

import math
from dataclasses import dataclass

from .opportunities import LISTING_SLOTS, Opportunity

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


def _terms(op: Opportunity, horizon: float):
    """(units held at once, capital, profit over horizon) for running op at full speed."""
    if op.your_per_day <= 0 or op.cost_unit <= 0 or op.profit_unit <= 0:
        return None
    units = max(1, math.ceil(op.your_per_day * min(horizon, 1.0)))
    sold = op.your_per_day * horizon
    return units, units * op.cost_unit, op.profit_unit * sold, sold


def _greedy(cands, budget, horizon, minutes_cap, limit):
    lines, left, minutes_left = [], budget, minutes_cap
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
        take = min(int(math.floor(units * frac)) or 1, units, int(left // op.cost_unit))
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
              play_hours: float = 8.0) -> Plan:
    """Best use of a budget within the market's limits: at most 10 items listed at once and
    5 minutes per listing, so your playing time caps how much you can sell per day."""
    minutes_cap = max(0.0, play_hours * 60 - OVERHEAD_MINUTES)
    cands = []
    for op in ops:
        t = _terms(op, horizon)
        if t and op.minutes_per_day > 0:
            cands.append((op,) + t)
    best = []
    # capital-bound and time-bound orderings; keep whichever earns more
    for key in (lambda c: c[3] / c[2], lambda c: c[3] / c[0].minutes_per_day):
        lines = _greedy(sorted(cands, key=key, reverse=True), budget, horizon, minutes_cap, limit)
        if sum(l.profit for l in lines) > sum(l.profit for l in best):
            best = lines
    return Plan(best, budget, horizon, sum(l.capital for l in best), sum(l.profit for l in best),
                sum(l.minutes for l in best), play_hours * 60)


@dataclass
class HoldTier:
    label: str
    amount: float
    profit_per_day: float


GRID = [250_000, 500_000, 750_000, 1e6, 1.5e6, 2e6, 3e6, 4e6, 5e6, 7.5e6, 10e6, 12.5e6, 15e6, 20e6, 25e6, 30e6]
TIERS = (("Low", 0.5, 5e6), ("Mid", 0.8, 15e6), ("High", 0.95, HOLD_CAP))


def holding_tiers(ops: list[Opportunity], horizon: float = 3.0, play_hours: float = 8.0) -> list[HoldTier]:
    """Low / mid / high amounts worth holding, from where extra kinah stops paying.

    Each tier is the smallest amount that earns its share (50% / 80% / 95%) of what
    30 million could earn, but never more than 5M / 15M / 30M: holding more than
    that is unrealistic for most players.
    """
    curve = [(b, make_plan(ops, b, horizon, play_hours=play_hours).profit_per_day) for b in GRID]
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
