"""Every way to make kinah, scored the same way so they can be compared.

Methods:
  Craft   - buy (or morph / NPC-buy / sub-craft) the inputs, craft, sell the result
  Resell  - buy listings that sit well below the item's usual price and sell them
            again in small lots at the usual price as buyers come

Speed matters as much as margin: kinah that comes back in a day can be spent
again, so the main ranking is profit per day, not profit per item.
"""
from __future__ import annotations

import math
from dataclasses import dataclass, field

from .crafting import CraftContext, CraftResult, batch_loss_probability
from .pricing import MIN_HISTORY_POINTS, ItemStats

SAFE, MODERATE, RISKY = "Safe", "Moderate", "Risky"

# Predicted returns are optimistic: in simulations on real price history, trades realised less
# than predicted, but in the same order. realistic = A*ln(1+predicted) + B, fitted on 77,375
# simulated trades on the EU servers Kaisinel and Zikel (a trade predicted at +100% realised about
# +76%, +50% about +41%, under +10% about nothing). NA servers realised a bit less (A 1.06, B -0.12)
# but the ranking of trades, which is what the plan uses, came out the same with either fit.
CALIBRATION = (1.23, -0.09)


def realistic_roi(pred_roi: float, calibration=CALIBRATION) -> float:
    a, b = calibration
    return a * math.log1p(max(pred_roi, -0.99)) + b


# On top of that, what kind of trade it is matters. Learned from 277,491 trades in 3,600 simulated
# 30-day runs on EU servers, keeping only effects that held on both Kaisinel and Zikel, then checked
# in 50,000 more runs (better worst-case results, slightly more profit):
#  - a Splendent used as the base for the next-tier craft made +28% / -48% where +291% / +24% was
#    expected: that use is counted at a quarter of its estimated value
#  - crafts needing a Splendent base made through combo procs realised ~60% of the estimate
#  - big price swings (over ±35%) ~74%, weapons ~75%
#  - consumables (potions, scrolls, food): in 501,120 more runs they made -39% / +5% / +3% / +81%
#    where +64% / +99% / +59% / +137% was predicted on the four servers that trade them -> ~40%
UPGRADE_KEEP = 0.25
FORMULA_FACTORS = {"combo_base": 0.6, "swings": 0.74, "weapons": 0.75, "consumables": 0.4}


def formula_roi(op) -> float:
    """Realistic return of an opportunity: the calibrated return, adjusted for the kind of trade."""
    c, st = op.craft, op.stats
    profit = op.profit_unit
    if c and c.combo_best == "craft":
        profit -= c.p_combo * c.combo_value * (1 - UPGRADE_KEEP)
    v = realistic_roi(profit / op.cost_unit if op.cost_unit > 0 else 0.0)
    if v <= 0:
        return v
    if c and any(l.source.how == "combo" for l in c.lines):
        v *= FORMULA_FACTORS["combo_base"]
    if st and (st.volatility or 0) > 0.35:
        v *= FORMULA_FACTORS["swings"]
    if st and st.section == "weapons":
        v *= FORMULA_FACTORS["weapons"]
    if st and st.section == "consumables":
        v *= FORMULA_FACTORS["consumables"]
    return v


def op_realistic(op) -> float:
    return op.real_roi
RISK_ORDER = {SAFE: 0, MODERATE: 1, RISKY: 2}

MARKET_SHARE = 0.3      # share of an item's daily sales you can expect to win as one more seller
MIN_MARGIN = 0.03       # ignore resells under 3% net margin
FAST_DAYS = 1.0         # "fast seller": a day's batch sells within this many days

# In-game market limits (from players): at most 10 listings up at once, and putting
# one listing up takes about 5 minutes. Buyers take whole listings and prefer small
# ones, so stackable items are listed in small lots.
LISTING_SLOTS = 10
LISTING_MINUTES = 5.0
CRAFT_SECONDS = 15.0
LISTINGS_PER_ITEM_DAY = 6   # aim for about this many small lots per item per day
MAX_LOT = 20
SINGLE_SECTIONS = {"armor", "weapons", "accessories", "skins"}  # never stacked: one item per listing


@dataclass
class Opportunity:
    method: str
    name: str
    key: str
    cost_unit: float            # kinah you spend per unit (per craft attempt for crafts)
    revenue_unit: float         # expected kinah back per unit, after tax and listing fee
    profit_unit: float
    profit_pess_unit: float     # if prices dip to the low end of their history
    market_per_day: float | None  # how many the whole market buys per day
    your_per_day: float         # units (attempts) you can expect to turn over per day
    sell_price: float           # price to list at
    buy_price: float | None
    risk: str = MODERATE
    reasons: list = field(default_factory=list)
    good: list = field(default_factory=list)
    craft: CraftResult | None = None
    stats: ItemStats | None = None
    outputs_per_unit: float = 1.0  # items to sell per unit bought (success x output qty for crafts)

    @property
    def sold_per_day(self) -> float:
        return self.your_per_day * self.outputs_per_unit

    @property
    def lot(self) -> int:
        """Items per listing: 1 for gear, small lots for stackables."""
        if self.stats and self.stats.section in SINGLE_SECTIONS:
            return 1
        return int(min(MAX_LOT, max(1, math.ceil(self.sold_per_day / LISTINGS_PER_ITEM_DAY))))

    @property
    def listings_per_day(self) -> float:
        return self.sold_per_day / self.lot

    @property
    def minutes_per_day(self) -> float:
        """Your time per day: putting listings up, plus crafting."""
        craft = self.your_per_day * CRAFT_SECONDS / 60.0 if self.method == "Craft" else 0.0
        return self.listings_per_day * LISTING_MINUTES + craft

    @property
    def profit_per_listing(self) -> float:
        return self.profit_day / self.listings_per_day if self.listings_per_day > 0 else 0.0

    @property
    def roi(self) -> float:
        return self.profit_unit / self.cost_unit if self.cost_unit > 0 else 0.0

    @property
    def real_roi(self) -> float:
        """What trades like this really returned in simulations (see CALIBRATION and formula_roi)."""
        return formula_roi(self)

    @property
    def real_profit_unit(self) -> float:
        return self.cost_unit * self.real_roi

    @property
    def real_profit_day(self) -> float:
        return self.real_profit_unit * self.your_per_day

    @property
    def profit_day(self) -> float:
        return self.profit_unit * self.your_per_day

    @property
    def batch(self) -> int:
        """Units to buy now: about one day of what you can sell."""
        return max(1, int(math.floor(self.your_per_day)))

    @property
    def sell_days(self) -> float | None:
        """Days until a one-day batch has sold (at least the wait for the first buyer)."""
        if self.your_per_day <= 0:
            return None
        return self.batch / self.your_per_day

    @property
    def fast(self) -> bool:
        return self.sell_days is not None and self.sell_days <= FAST_DAYS

    @property
    def capital(self) -> float:
        return self.cost_unit * self.batch


def speed_text(days: float | None) -> str:
    if days is None:
        return "unknown"
    hours = days * 24
    if hours < 1:
        return "< 1 hour"
    if hours < 24:
        return f"~{hours:.0f} h"
    return f"~{days:.1f} days"


def _sales_reasons(st: ItemStats, reasons: list, good: list):
    if st.sales_per_day is None:
        reasons.append("no sales evidence yet (refresh a few times to build history)")
    elif st.sales_basis.startswith("market sales"):
        good.append(f"real sales data: ~{st.sales_per_day:.1f}/day")
    elif st.sales_hours < 3:
        reasons.append(f"sales rate based on only {st.sales_hours:.0f} h of data")


def _supply_notes(st: ItemStats, reasons: list, good: list):
    if not st.supply_kind:
        return
    left = st.supply_hours_left or 0
    if left <= 24:
        reasons.append(f"its {st.supply_kind.lower()} supply request ends in {left:.0f} h: buyers drop after that")
    else:
        good.append(f"needed for a {st.supply_kind.lower()} supply request: extra buyers for {left / 24:.1f} more days")


# Learned from two million-run searches (999,360 runs on the EU servers Kaisinel and Zikel, and
# 1,002,240 runs on five servers incl. NA); only rules that held in both regions are kept:
#  - trades predicted under +20% are not worth a listing slot -> Risky
#  - "loses money if the price falls to its recent low" made about nothing (EU -8%, NA -2%) -> Risky
#  - "only profitable when the Splendent combo hits" swings more, but skipping those trades cost
#    16M in EU and 8M in NA over 50 days -> Moderate (never Safe)
#  - big price swings, falling prices and failed-craft risk alone don't make a trade worse
AVOID = ("loses money if the price falls",
         "the price is a guess")  # a result with almost no listings and no sales: never rely on it
SWINGY = ("only profitable when the Splendent combo hits",)
MIN_PRED_ROI = 0.20
SAFE_REAL_ROI = 0.15


def classify(op: Opportunity, st: ItemStats, extra_risky: list, extra_safe_ok: bool) -> None:
    reasons, good = op.reasons, op.good
    _sales_reasons(st, reasons, good)
    _supply_notes(st, reasons, good)
    if st.volatility is not None and st.volatility > 0.35:
        reasons.append(f"price swings ±{st.volatility:.0%}")
    if op.profit_pess_unit < 0:
        reasons.append("loses money if the price falls to its recent low")
    if op.sell_days is not None and op.sell_days > 7:
        reasons.append(f"slow: {speed_text(op.sell_days)} to sell")
    if st.history_points < MIN_HISTORY_POINTS and not st.units_sold_28d:
        reasons.append("little price history")
    if op.roi < MIN_PRED_ROI:
        reasons.append(f"thin margin (+{op.roi:.0%} predicted): under +20% wasn't worth a listing slot in simulations")
    reasons.extend(extra_risky)

    hard = (st.sales_per_day is None or (op.sell_days or 99) > 7 or op.roi < MIN_PRED_ROI
            or any(a in r for r in reasons for a in AVOID))
    if hard:
        op.risk = RISKY
        return
    swingy = any(a in r for r in reasons for a in SWINGY)
    safe = (not swingy and op.real_roi >= SAFE_REAL_ROI and (st.sales_per_day or 0) >= 2
            and (st.history_points >= 24 or st.units_sold_28d > 0)
            and st.listings >= 5 and extra_safe_ok and (op.sell_days or 99) <= 3)
    if safe:
        good.append(f"realistic return +{op.real_roi:.0%} (after the usual gap between predicted and real)")
    op.risk = SAFE if safe else MODERATE


UNKNOWN_OUTLET = 0.5  # Splendent pieces per day assumed sellable when there's no sales evidence


def combo_outlet(res: CraftResult, ctx: CraftContext) -> float:
    """Splendent pieces per day you can turn into kinah: your share of their own sales, or of the
    next-tier item's sales when the app says to craft with them."""
    if res.combo_best == "craft" and res.combo_use_outlet:
        key, success, out_qty, qty = res.combo_use_outlet
        ust = ctx.stats.get(key)
        if ust and ust.sales_per_day and success > 0:
            return ust.sales_per_day * MARKET_SHARE / (success * out_qty) * qty
        return UNKNOWN_OUTLET
    cst = ctx.stats.get(res.combo_key) if res.combo_key else None
    if cst and cst.sales_per_day:
        return cst.sales_per_day * MARKET_SHARE
    return UNKNOWN_OUTLET


def craft_opportunity(res: CraftResult, ctx: CraftContext) -> Opportunity | None:
    if not res.priced or res.profit <= 0 or res.missing:
        return None  # no price, no profit, or a cost we can't complete (an ingredient nobody sells)
    st = ctx.stats[res.out_key]
    outputs_per_attempt = res.success * res.recipe.out_qty
    market = st.sales_per_day
    your_units = (market or 0) * MARKET_SHARE
    attempts_day = your_units / outputs_per_attempt if outputs_per_attempt > 0 else 0
    revenue, pess = res.ev, res.ev_pessimistic
    if res.p_combo > 0 and attempts_day > 0:
        # Splendent pieces only count up to what their market (or the next-tier item's) takes per day
        outlet = combo_outlet(res, ctx)
        pieces = attempts_day * res.p_combo
        if pieces > outlet:
            plain_part = res.p_plain * res.plain_value - res.cost
            if plain_part < 0:   # only the combos pay: craft no more than the Splendent market takes
                attempts_day = outlet / res.p_combo
            else:                # normal results pay on their own: extra Splendents are worth nothing here
                keep_share = outlet / pieces
                revenue = res.ev - res.p_combo * res.combo_value * (1 - keep_share)
                pess = res.ev_pessimistic - res.p_combo * res.combo_value * (1 - keep_share)
    if revenue - res.cost <= 0:
        return None
    op = Opportunity(
        method="Craft", name=res.name, key=res.recipe.id, cost_unit=res.cost, revenue_unit=revenue,
        profit_unit=revenue - res.cost, profit_pess_unit=pess - res.cost, market_per_day=market,
        your_per_day=attempts_day, sell_price=st.expected_sell, buy_price=None, craft=res, stats=st,
        outputs_per_unit=outputs_per_attempt,
    )
    risky = []
    if res.profit_plain_only <= 0:
        risky.append("only profitable when the Splendent combo hits (luck)")
    loss10 = batch_loss_probability(res, 10)
    if loss10 > 0.35:
        risky.append(f"{loss10:.0%} chance 10 crafts lose money (failures)")
    for line in res.lines:
        if line.source.how in ("buy", "bulk"):
            ist = ctx.stats.get(line.key)
            if ist and ist.listings < 3:
                risky.append(f"ingredient {line.name} has only {ist.listings} listing(s)")
    if st.falling:
        risky.append(f"{res.name} price falling: {st.trend_short:+.0%} in the last day")
    if not res.price_ok:
        risky.append(f"only {st.listings} listed and no sales record: the price is a guess")
    if res.profit_plain_only <= 0 and not res.combo_use_ok:
        risky.append(f"its profit rests on {res.combo_use_name}, which has no sales record yet: the price is a guess")
    elif st.listings < 3:
        risky.append(f"only {st.listings} listed right now: its price comes from past sales")
    for n in res.notes:
        op.reasons.append(n)
    safe_ok = res.success >= 0.85 and loss10 < 0.1 and not (st.trend_7d is not None and st.trend_7d < -0.1)
    classify(op, st, risky, safe_ok)
    return op


def resell_opportunity(st: ItemStats, keep: float) -> Opportunity | None:
    if not st.trusted or not st.ask or not st.ref:
        return None
    if st.history_points < MIN_HISTORY_POINTS and not (st.units_sold_28d and st.sold_avg):
        return None  # never guess "usual price" without history
    target = st.expected_target  # never above the last 24 h, and a falling price keeps falling
    buy = st.ask
    if st.bulk_unit and st.bulk_unit > buy:
        buy_batch = st.bulk_unit
    else:
        buy_batch = buy
    revenue = target * keep
    profit = revenue - buy_batch
    if profit <= buy_batch * MIN_MARGIN:
        return None
    pess = min(st.pessimistic or target * 0.85, target) * keep - buy_batch  # the low case is never above the target
    your = (st.sales_per_day or 0) * MARKET_SHARE
    op = Opportunity(
        method="Resell", name=st.name, key=st.key, cost_unit=buy_batch, revenue_unit=revenue, profit_unit=profit,
        profit_pess_unit=pess, market_per_day=st.sales_per_day, your_per_day=your, sell_price=target,
        buy_price=buy_batch, stats=st,
    )
    risky = []
    if st.falling:
        risky.append(f"price falling: {st.trend_short:+.0%} in the last day (the 'dip' may be the new price)")
    if st.trend_7d is not None and st.trend_7d < -0.15:
        risky.append(f"price falling: {st.trend_7d:+.0%} over 7 days (the dip may be the new normal)")
    classify(op, st, risky, extra_safe_ok=(st.trend_7d is None or st.trend_7d > -0.05)
             and st.trend_short is not None and st.trend_short > -0.02)
    return op


def all_opportunities(ctx: CraftContext, crafts: list[CraftResult]) -> list[Opportunity]:
    ops = []
    best_craft: dict[str, Opportunity] = {}
    for res in crafts:
        op = craft_opportunity(res, ctx)
        if op and op.real_profit_unit > 0:  # one row per item: its best recipe
            cur = best_craft.get(res.out_key)
            if cur is None or op.real_profit_day > cur.real_profit_day:
                best_craft[res.out_key] = op
    ops.extend(best_craft.values())
    for st in ctx.stats.values():
        if st.name and "(Bound)" in st.name:
            continue
        op = resell_opportunity(st, ctx.keep)
        if op and op.real_profit_unit > 0:
            ops.append(op)
    ops.sort(key=lambda o: o.real_profit_day, reverse=True)
    return ops
