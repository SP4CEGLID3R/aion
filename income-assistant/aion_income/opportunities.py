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
        """What trades like this really returned in simulations (see CALIBRATION)."""
        return realistic_roi(self.roi)

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
AVOID = ("loses money if the price falls",)
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


def craft_opportunity(res: CraftResult, ctx: CraftContext) -> Opportunity | None:
    if res.profit <= 0:
        return None
    st = ctx.stats[res.out_key]
    outputs_per_attempt = res.success * res.recipe.out_qty
    market = st.sales_per_day
    your_units = (market or 0) * MARKET_SHARE
    attempts_day = your_units / outputs_per_attempt if outputs_per_attempt > 0 else 0
    op = Opportunity(
        method="Craft", name=res.name, key=res.recipe.id, cost_unit=res.cost, revenue_unit=res.ev,
        profit_unit=res.profit, profit_pess_unit=res.profit_pessimistic, market_per_day=market,
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
    if st.listings < 3:
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
    for res in crafts:
        op = craft_opportunity(res, ctx)
        if op:
            ops.append(op)
    for st in ctx.stats.values():
        if st.name and "(Bound)" in st.name:
            continue
        op = resell_opportunity(st, ctx.keep)
        if op:
            ops.append(op)
    ops.sort(key=lambda o: o.real_profit_day, reverse=True)
    return ops
