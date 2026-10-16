"""Crafting economics: success chance from your levels, cheapest way to get each input,
expected profit per craft.

Success model (community measurements of the in-game "Expected Probability" tab;
NC publishes no official table):
  * chance at your level = base + (max - base) * curve(proficiency - recipe level)
  * base/max by recipe kind, e.g. basic gear 74% -> 93.1%, Expert's 76.8% -> 94.6%,
    Artisan's 79.6% -> 96.8%, Dragon Lord gear 67.5% -> 94.6%,
    potions/scrolls/food 72.88% -> 90.89%, material processing 95.2% -> 99.2%
  * the curve follows the published per-level table of a recipe (Superior Healing
    Potion: +0 72.88, +5 75.55, +10 79.60, +15 82.98, +20 83.76, +30 87.26,
    +40 88.11, +60 90.89)
  * a success has a `combo_chance` (usually 25%) to try for the Splendent version;
    that try succeeds at the combo rate (Dragon Lord: 80% -> 97.4%, otherwise the
    normal rate); a failed try still gives the normal item.
  * failed crafts consume the materials.
Use the per-recipe override in the app when the game shows you a different number.
"""
from __future__ import annotations

from dataclasses import dataclass, field

from .recipes import KINA_KEY, PROFESSIONS, Recipe, RecipeBook

# proficiency above the recipe level -> share of the way from base to max chance
CURVE = [(0, 0.0), (5, 0.148), (10, 0.373), (15, 0.56), (20, 0.604), (30, 0.798), (40, 0.845), (50, 0.92), (60, 1.0)]

GEAR = ("weapon", "armor", "accessory")


def curve(gap: float) -> float:
    if gap <= 0:
        return 0.0
    for (x0, y0), (x1, y1) in zip(CURVE, CURVE[1:]):
        if gap <= x1:
            return y0 + (y1 - y0) * (gap - x0) / (x1 - x0)
    return 1.0


def recipe_kind(book: RecipeBook, r: Recipe) -> str:
    name = book.name(r.out)
    mc = book.category(r.out)
    if mc in GEAR:
        if r.tier == "intermediate" or "Dragon" in name or "Archon" in name:
            return "dragon"
        if name.startswith("Artisan's"):
            return "artisan"
        if name.startswith("Expert's"):
            return "expert"
        return "basic"
    if mc == "usable":
        return "consumable"
    return "processing"


RATES = {  # kind -> (chance at +0, best chance)
    "basic": (0.74, 0.931),
    "expert": (0.768, 0.946),
    "artisan": (0.796, 0.968),
    "dragon": (0.675, 0.946),
    "consumable": (0.7288, 0.9089),
    "processing": (0.952, 0.992),
}


def success_chance(book: RecipeBook, r: Recipe, proficiency: int) -> float | None:
    """None when the recipe is above your level (you cannot craft it)."""
    if proficiency <= 0 or proficiency < r.level:
        return None
    base, top = RATES[recipe_kind(book, r)]
    return base + (top - base) * curve(proficiency - r.level)


def combo_success(book: RecipeBook, r: Recipe, proficiency: int, normal: float) -> float:
    if recipe_kind(book, r) == "dragon":
        return 0.80 + (0.974 - 0.80) * curve(proficiency - r.level)
    return normal


@dataclass
class Source:
    cost: float           # kinah per unit
    how: str              # "buy" | "bulk" | "npc" | "morph" | "craft" | "kina"
    detail: str = ""
    via: str | None = None  # recipe id when crafted, odyle key when morphed


@dataclass
class Line:
    key: str
    name: str
    qty: int
    unit: float
    source: Source


@dataclass
class CraftResult:
    recipe: Recipe
    name: str
    profession: str
    success: float
    combo_rate: float
    cost: float                 # expected kinah spent per craft attempt
    lines: list
    p_fail: float
    p_plain: float
    p_combo: float
    plain_value: float          # net (after tax and fee) per plain result
    combo_value: float
    ev: float                   # expected net revenue per attempt
    profit: float
    ev_pessimistic: float
    profit_pessimistic: float
    profit_plain_only: float    # profit if no combo ever happens
    out_key: str
    combo_key: str | None
    notes: list = field(default_factory=list)
    priced: bool = True            # the result has a market price at all (else profit is unknown)
    price_ok: bool = True          # ... and enough listings or sales to trust it
    combo_name: str = ""           # the Splendent version a combo proc gives
    combo_sell: float | None = None  # net kinah per Splendent if you sell it (None: nobody buys it)
    combo_use: float | None = None   # net kinah per Splendent if you use it to craft the next tier
    combo_use_name: str = ""       # ... that next-tier item
    combo_best: str = ""           # "sell" or "craft"
    combo_use_outlet: tuple = ()   # (next-tier item key, its success, its output qty, Splendents per craft)

    @property
    def avg_cost_item(self) -> float:
        """Average kinah per finished item, failed attempts included."""
        made = self.success * self.recipe.out_qty
        return self.cost / made if made > 0 else float("inf")


class CraftContext:
    def __init__(self, book: RecipeBook, stats: dict, levels: dict, faction: str,
                 keep: float, overrides: dict | None = None):
        self.book = book
        self.stats = stats          # item key -> ItemStats
        self.levels = levels        # profession -> proficiency
        self.faction = faction
        self.keep = keep            # share of the sale price you keep after tax and listing fee
        self.overrides = overrides or {}
        self.recipes = [r for r in book.for_faction(faction)]
        self.by_out = {}
        for r in self.recipes:
            self.by_out.setdefault(r.out, []).append(r)
        self.acq: dict[str, Source] = {}
        self._compute_acquire()

    # ── chances ──────────────────────────────────────────────────────────

    def chance(self, r: Recipe) -> float | None:
        ov = self.overrides.get(r.id)
        prof = int(self.levels.get(r.profession, 0) or 0)
        if prof < r.level or prof <= 0:
            return None
        if ov:
            return max(0.01, min(1.0, float(ov) / 100.0))
        return success_chance(self.book, r, prof)

    # ── where inputs come from ───────────────────────────────────────────

    def market_buy(self, key: str) -> Source | None:
        st = self.stats.get(key)
        if not st or not st.ask:
            return None
        if st.listings < 1:
            return None
        price = st.ask
        if st.spike and st.ref:
            price = max(price, st.ref)
        if st.bulk_unit and st.bulk_unit > price:
            # crafting needs many units: the cheapest listing runs out, pay the batch average
            return Source(st.bulk_unit, "bulk", f"buy on market (avg for {st.bulk_units or 'a batch of'} units)")
        return Source(price, "buy", "buy on market")

    def npc_buy(self, key: str) -> Source | None:
        name = self.book.name(key)
        if name in self.book.npc_prices:
            est = " (estimated price)" if name in self.book.npc_estimated else ""
            return Source(self.book.npc_prices[name], "npc", f"NPC shop{est}")
        return None

    def base_source(self, key: str) -> Source | None:
        if key == KINA_KEY:
            return Source(1.0, "kina", "kinah")
        cands = [s for s in (self.market_buy(key), self.npc_buy(key)) if s]
        return min(cands, key=lambda s: s.cost) if cands else None

    def _offer(self, key: str, alt: str | None):
        best = self.acq.get(key)
        used = key
        if alt:
            b = self.acq.get(alt)
            if b and (best is None or b.cost < best.cost):
                best, used = b, alt
        return used, best

    def run_inputs(self, r: Recipe, free: frozenset = frozenset()):
        """Cost per attempt and ingredient lines; `free` items are ones you already own (e.g. a
        Splendent from a combo proc) and cost nothing here."""
        cost = float(r.gold or 0)
        lines = []
        for key, qty, alt in r.inputs:
            if key in free:
                lines.append(Line(key, self.book.name(key), qty, 0.0, Source(0.0, "own", "your Splendent from a combo")))
                continue
            used, src = self._offer(key, alt)
            if src is None:
                return None
            if src.how == "bulk" and qty <= 3:
                # a few units come from the cheapest listings, no batch average needed
                st = self.stats.get(used)
                if st and st.ask:
                    src = Source(max(st.ask, st.ref) if st.spike and st.ref else st.ask, "buy", "buy on market")
            cost += src.cost * qty
            lines.append(Line(used, self.book.name(used), qty, src.cost, src))
        return cost, lines

    def _compute_acquire(self):
        keys = set()
        for r in self.recipes:
            keys.add(r.out)
            for k, _, alt in r.inputs:
                keys.add(k)
                if alt:
                    keys.add(alt)
        for k, (od, _) in self.book.morph.items():
            keys.add(k)
            keys.add(od)
        for k in keys:
            s = self.base_source(k)
            if s:
                self.acq[k] = s
        usable = [r for r in self.recipes if self.chance(r)]
        # Relax until no unit gets cheaper (handles chains: ore -> ingot -> gear).
        for _ in range(40):
            changed = False
            for k, (od, qty) in self.book.morph.items():
                o = self.acq.get(od)
                if not o or o.how not in ("buy", "bulk"):
                    continue  # morph only from Odyle you can buy
                cost = o.cost * qty
                cur = self.acq.get(k)
                if cur is None or cost < cur.cost - 1e-9:
                    self.acq[k] = Source(cost, "morph", f"Substance Morph from {self.book.name(od)}", od)
                    changed = True
            for r in usable:
                run = self.run_inputs(r)
                if not run:
                    continue
                s = self.chance(r)
                per_unit = run[0] / (s * r.out_qty)
                cur = self.acq.get(r.out)
                if cur is None or per_unit < cur.cost - 1e-9:
                    self.acq[r.out] = Source(per_unit, "craft", f"craft yourself (lv {r.level}, {s:.0%})", r.id)
                    changed = True
            if not changed:
                break

    # ── evaluating one recipe ────────────────────────────────────────────

    def evaluate(self, r: Recipe, base_values: dict | None = None,
                 free: frozenset = frozenset()) -> CraftResult | None:
        """Expected cost and return of one attempt. base_values: Splendent key -> (net kinah per
        piece when used to craft the next tier, same at low-end prices, that item's name)."""
        s = self.chance(r)
        if not s:
            return None
        run = self.run_inputs(r, free)
        if not run:
            return None
        cost, lines = run
        out = self.stats.get(r.out)
        name = self._name(r.out)
        prof = PROFESSIONS.get(r.profession, r.profession)
        if not out or out.sell_price is None or out.expected_sell is None:
            # nobody has listed it in the history we have: show what it costs, not a profit
            return CraftResult(
                recipe=r, name=name, profession=prof, success=s, combo_rate=0.0, cost=cost, lines=lines,
                p_fail=1 - s, p_plain=s, p_combo=0.0, plain_value=0.0, combo_value=0.0, ev=0.0, profit=0.0,
                ev_pessimistic=0.0, profit_pessimistic=0.0, profit_plain_only=0.0, out_key=r.out, combo_key=None,
                notes=["no market price for it yet"], priced=False, price_ok=False)
        cs = combo_success(self.book, r, int(self.levels.get(r.profession, 0)), s)
        cp = r.combo_chance if r.combo else 0.0
        combo_st = self.stats.get(r.combo) if r.combo else None
        # a Splendent is worth what it sells for (only if people really buy it), or what it's worth
        # as the base for the next-tier craft, whichever is more
        sell = sell_pess = None
        if combo_st and combo_st.trusted and (combo_st.sales_per_day or 0) > 0 and combo_st.expected_sell:
            sell, sell_pess = combo_st.expected_sell * self.keep, (combo_st.sell_pessimistic or 0) * self.keep
        use, use_pess, use_name, outlet = ((base_values or {}).get(r.combo, (None, None, "", ())) if r.combo
                                           else (None, None, "", ()))
        if use is not None and use <= 0:
            use = use_pess = None
        best = ""
        combo_v = combo_pess = 0.0
        if use is not None and use > (sell or 0):
            best, combo_v, combo_pess = "craft", use, max(0.0, use_pess or 0.0)
        elif sell is not None:
            best, combo_v, combo_pess = "sell", sell, sell_pess
        combo_ok = combo_v > 0
        p_combo = s * cp * cs if combo_ok else 0.0
        p_plain = s - p_combo
        p_fail = 1 - s
        plain_v = out.expected_sell * r.out_qty * self.keep
        plain_pess = (out.sell_pessimistic or 0) * r.out_qty * self.keep
        ev = p_plain * plain_v + p_combo * combo_v
        ev_pess = p_plain * plain_pess + p_combo * combo_pess
        notes = []
        if r.combo and not combo_ok:
            notes.append("Splendent version has no reliable price or use, counted as a normal item")
        if any(l.source.how == "npc" and "estimated" in l.source.detail for l in lines):
            notes.append("uses an NPC item with an estimated price")
        return CraftResult(
            recipe=r, name=name, profession=prof,
            success=s, combo_rate=cs, cost=cost, lines=lines, p_fail=p_fail, p_plain=p_plain, p_combo=p_combo,
            plain_value=plain_v, combo_value=combo_v, ev=ev, profit=ev - cost, ev_pessimistic=ev_pess,
            profit_pessimistic=ev_pess - cost, profit_plain_only=s * plain_v - cost,
            out_key=r.out, combo_key=r.combo if combo_ok else None, notes=notes,
            price_ok=out.trusted, combo_name=self._name(r.combo) if r.combo else "",
            combo_sell=sell, combo_use=use, combo_use_name=use_name, combo_best=best,
            combo_use_outlet=outlet if best == "craft" else (),
        )

    def _name(self, key: str) -> str:
        n = self.book.name(key)
        st = self.stats.get(key)
        return st.name if n == key and st and st.name else n

    def base_values(self) -> dict:
        """What one Splendent is worth as the base of the next-tier craft it goes into: that craft's
        normal result after its other ingredients, failures and tax, for recipes you can make. The
        next tier's own combo luck is left out on purpose, so values never pile up along a chain."""
        combos = {r.combo for r in self.recipes if r.combo}
        values: dict = {}
        for r in self.recipes:
            for key, qty, _ in r.inputs:
                if key not in combos or qty <= 0:
                    continue
                res = self.evaluate(r, None, free=frozenset({key}))
                if not res or not res.priced or not res.price_ok:
                    continue
                out = self.stats.get(r.out)
                plain_pess = (out.sell_pessimistic or 0) * r.out_qty * self.keep if out else 0.0
                v = res.profit_plain_only / qty
                vp = (res.success * plain_pess - res.cost) / qty
                if v > 0 and (key not in values or v > values[key][0]):
                    values[key] = (v, vp, res.name, (r.out, res.success, r.out_qty, qty))
        return values

    def evaluate_all(self) -> list[CraftResult]:
        bases = self.base_values()
        best: dict[tuple, CraftResult] = {}
        for r in self.recipes:
            res = self.evaluate(r, bases)
            if not res:
                continue
            k = (r.out, r.level)  # the same item may have several recipes: keep the best
            if k not in best or res.profit > best[k].profit:
                best[k] = res
        return sorted(best.values(), key=lambda c: c.profit, reverse=True)

    def cheapest_source(self, key: str) -> Source | None:
        return self.acq.get(key)


def batch_loss_probability(res: CraftResult, n: int) -> float:
    """Chance that n crafts lose money overall: each attempt fails, gives the plain result or
    gives the Splendent (combo) result, so a combo-luck craft isn't shown as a certain loss."""
    if n <= 0:
        return 0.0
    from math import factorial
    pf, pp, pc = max(0.0, res.p_fail), max(0.0, res.p_plain), max(0.0, res.p_combo)
    total = pf + pp + pc
    if total <= 0:
        return 1.0
    pf, pp, pc = pf / total, pp / total, pc / total
    spend = res.cost * n
    p = 0.0
    for a in range(n + 1):          # plain results
        for b in range(n - a + 1):  # combo results
            if a * res.plain_value + b * res.combo_value < spend - 1e-9:
                f = n - a - b
                p += factorial(n) / (factorial(a) * factorial(b) * factorial(f)) * pp ** a * pc ** b * pf ** f
    return min(1.0, p)
