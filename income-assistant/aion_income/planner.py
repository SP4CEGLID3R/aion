"""Spend a budget across opportunities, and suggest how much kinah to hold.

Fast sellers return their kinah quickly and it can be spent again, so each
opportunity is valued by what it earns over the whole horizon per kinah it ties
up: you buy about one day of what you can sell, sell it, re-buy, repeat.
"""
from __future__ import annotations

import math
from dataclasses import dataclass

from .opportunities import Opportunity

HOLD_CAP = 30_000_000  # holding more than this is unrealistic for most players


@dataclass
class PlanLine:
    op: Opportunity
    units: int          # buy / craft this many now
    capital: float      # kinah tied up now
    profit: float       # expected profit over the horizon (re-buying as it sells)
    sold_over_horizon: float


@dataclass
class Plan:
    lines: list
    budget: float
    horizon: float
    capital: float
    profit: float

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


def make_plan(ops: list[Opportunity], budget: float, horizon: float = 3.0, limit: int = 25) -> Plan:
    cands = []
    for op in ops:
        t = _terms(op, horizon)
        if t:
            units, capital, profit, sold = t
            cands.append((profit / capital, op, units, capital, profit, sold))
    cands.sort(key=lambda c: c[0], reverse=True)
    lines, left = [], budget
    used_keys = set()
    for _, op, units, capital, profit, sold in cands:
        if left < op.cost_unit:
            continue
        sell_key = op.stats.key if op.stats else (op.method, op.key)
        if sell_key in used_keys:
            continue  # buyers of one item are shared: don't count them twice
        take = min(units, int(left // op.cost_unit))
        if take <= 0:
            continue
        frac = take / units
        lines.append(PlanLine(op, take, take * op.cost_unit, profit * frac, sold * frac))
        used_keys.add(sell_key)
        left -= take * op.cost_unit
        if len(lines) >= limit:
            break
    return Plan(lines, budget, horizon, sum(l.capital for l in lines), sum(l.profit for l in lines))


@dataclass
class HoldTier:
    label: str
    amount: float
    profit_per_day: float


GRID = [250_000, 500_000, 750_000, 1e6, 1.5e6, 2e6, 3e6, 4e6, 5e6, 7.5e6, 10e6, 12.5e6, 15e6, 20e6, 25e6, 30e6]
TIERS = (("Low", 0.5, 5e6), ("Mid", 0.8, 15e6), ("High", 0.95, HOLD_CAP))


def holding_tiers(ops: list[Opportunity], horizon: float = 3.0) -> list[HoldTier]:
    """Low / mid / high amounts worth holding, from where extra kinah stops paying.

    Each tier is the smallest amount that earns its share (50% / 80% / 95%) of what
    30 million could earn, but never more than 5M / 15M / 30M: holding more than
    that is unrealistic for most players.
    """
    curve = [(b, make_plan(ops, b, horizon, limit=200).profit_per_day) for b in GRID]
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
