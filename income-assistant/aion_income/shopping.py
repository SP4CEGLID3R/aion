"""Shopping list: every material to buy for the trades you ticked, with quantities and prices.

Ingredients you craft yourself (ore -> ingot), Splendent bases you get from combo procs and
Odyle you morph are broken down into what you actually buy. Expected quantities include
failed crafts, so they are averages: buy a little extra if you want to be sure.
"""
from __future__ import annotations

import math
from dataclasses import dataclass, field

from .crafting import CraftContext, combo_success
from .opportunities import Opportunity


@dataclass
class Material:
    key: str
    name: str
    qty: float = 0.0
    unit: float = 0.0
    where: str = ""

    @property
    def total(self) -> float:
        return self.qty * self.unit

    @property
    def buy_qty(self) -> int:
        return int(math.ceil(self.qty - 1e-9))


@dataclass
class ShoppingList:
    materials: dict = field(default_factory=dict)   # key -> Material
    fees: float = 0.0                                # crafting fees in kinah
    byproducts: dict = field(default_factory=dict)   # key -> (name, qty): normal results made on the way
    attempts: dict = field(default_factory=dict)     # name -> crafts you'll make

    @property
    def total(self) -> float:
        return sum(m.buy_qty * m.unit for m in self.materials.values()) + self.fees

    def rows(self) -> list[Material]:
        return sorted(self.materials.values(), key=lambda m: -m.buy_qty * m.unit)

    def as_text(self) -> str:
        lines = [f"{m.buy_qty:>7} x {m.name:<40} {m.unit:>12,.0f} = {m.buy_qty * m.unit:>14,.0f}   ({m.where})"
                 for m in self.rows()]
        if self.fees:
            lines.append(f"{'':>7}   {'Crafting fees':<40} {'':>12}   {self.fees:>14,.0f}")
        lines.append(f"{'':>7}   {'TOTAL':<40} {'':>12}   {self.total:>14,.0f}")
        return "\n".join(lines)


class _Builder:
    MAX_DEPTH = 6

    def __init__(self, ctx: CraftContext):
        self.ctx = ctx
        self.by_id = {r.id: r for r in ctx.recipes}
        self.out = ShoppingList()

    def buy(self, key, qty, unit, where):
        m = self.out.materials.get(key)
        if m is None:
            m = self.out.materials[key] = Material(key, self.ctx.book.name(key), 0.0, unit, where)
        m.qty += qty

    def craft(self, r, attempts, depth):
        self.out.attempts[self.ctx.book.name(r.out)] = self.out.attempts.get(self.ctx.book.name(r.out), 0) + attempts
        self.out.fees += float(r.gold or 0) * attempts
        run = self.ctx.run_inputs(r)
        if not run:
            return
        for line in run[1]:
            self.need(line.key, line.qty * attempts, line.source, depth + 1)

    def need(self, key, qty, src, depth=0):
        if qty <= 0 or src is None:
            return
        how = src.how
        if depth > self.MAX_DEPTH or how in ("buy", "bulk", "npc"):
            self.buy(key, qty, src.cost, src.detail)
        elif how == "kina":
            self.out.fees += qty
        elif how == "morph":
            od, per = self.ctx.book.morph.get(key, (src.via, 1))
            od_src = self.ctx.cheapest_source(od)
            self.buy(od, qty * per, od_src.cost if od_src else src.cost / max(per, 1),
                     f"for Substance Morph into {self.ctx.book.name(key)}")
        elif how == "craft":
            r = self.by_id.get(src.via)
            s = self.ctx.chance(r) if r else None
            if not r or not s:
                self.buy(key, qty, src.cost, src.detail)
                return
            self.craft(r, qty / (s * r.out_qty), depth)
        elif how == "combo":
            r = self.by_id.get(src.via)
            s = self.ctx.chance(r) if r else None
            if not r or not s:
                self.buy(key, qty, src.cost, src.detail)
                return
            p = s * r.combo_chance * combo_success(self.ctx.book, r, int(self.ctx.levels.get(r.profession, 0)), s)
            attempts = qty / p
            self.craft(r, attempts, depth)
            name, made = self.out.byproducts.get(r.out, (self.ctx.book.name(r.out), 0.0))
            self.out.byproducts[r.out] = (name, made + (s - p) * r.out_qty * attempts)
        # "own": already yours, nothing to buy


def shopping_list(ctx: CraftContext, picks: list[tuple[Opportunity, int]]) -> ShoppingList:
    """picks: (opportunity, units) - craft attempts for crafts, items to buy for resells."""
    b = _Builder(ctx)
    for op, units in picks:
        if units <= 0:
            continue
        if op.method == "Craft" and op.craft:
            b.craft(op.craft.recipe, units, 0)
        else:
            b.buy(op.stats.key if op.stats else op.key, units, op.buy_price or op.cost_unit, "buy on market to resell")
    return b.out
