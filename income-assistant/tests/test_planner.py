from aion_income.opportunities import RISKY, SAFE, Opportunity, resell_opportunity
from aion_income.planner import HOLD_CAP, holding_tiers, make_plan
from aion_income.pricing import ItemStats


def op(name, cost, profit, per_day, risk=SAFE):
    st = ItemStats(key=name, name=name, ask=cost, listings=30)
    return Opportunity(method="Resell", name=name, key=name, cost_unit=cost, revenue_unit=cost + profit,
                       profit_unit=profit, profit_pess_unit=profit / 2, market_per_day=per_day / 0.3,
                       your_per_day=per_day, sell_price=cost + profit, buy_price=cost, risk=risk, stats=st)


def test_plan_never_exceeds_budget():
    ops = [op(f"i{i}", 10_000 + i * 997, 2_000, 50) for i in range(30)]
    for budget in (5_000, 123_456, 1e6, 5e6):
        p = make_plan(ops, budget, 3)
        assert p.capital <= budget + 1e-6
        assert all(l.units >= 1 for l in p.lines)


def test_fast_small_margin_beats_slow_big_margin():
    fast = op("fast", 100_000, 15_000, per_day=20)    # 15% but sells 20/day
    slow = op("slow", 100_000, 30_000, per_day=0.2)   # 30% but one every 5 days
    p = make_plan([slow, fast], 20_000_000, 3)
    by = {l.op.name: l.profit for l in p.lines}
    assert by["fast"] > 10 * by.get("slow", 0)   # the fast seller earns far more over the period
    assert p.profit > 30_000


def test_one_line_per_sold_item():
    a = op("same", 1000, 200, 10)
    b = op("same", 1000, 150, 10)
    b.method = "Craft"
    p = make_plan([a, b], 1e7, 3)
    assert len(p.lines) == 1


def test_holding_tiers_realistic_and_increasing():
    ops = [op(f"i{i}", 50_000 + i * 1000, 6_000, 40) for i in range(40)]
    tiers = holding_tiers(ops, 3)
    assert [t.label for t in tiers] == ["Low", "Mid", "High"]
    amounts = [t.amount for t in tiers]
    assert amounts == sorted(amounts) and len(set(amounts)) == 3
    assert amounts[-1] <= HOLD_CAP
    assert all(t.profit_per_day > 0 for t in tiers)


def test_holding_tiers_capped_for_huge_markets():
    ops = [op(f"i{i}", 2_000_000, 600_000, 50) for i in range(50)]
    assert max(t.amount for t in holding_tiers(ops, 3)) <= HOLD_CAP


def test_no_opportunities():
    assert holding_tiers([], 3) == []
    assert make_plan([], 1e6).lines == []


def test_resell_needs_history_and_margin_after_tax():
    st = ItemStats(key="k", name="Odyle", ask=1000, listings=40, ref=1200, ref_basis="t", history_points=48,
                   sales_per_day=30, sales_hours=24, volatility=0.05, pessimistic=1150, reversion=1.0)
    o = resell_opportunity(st, keep=0.88)
    assert o and o.profit_unit == 1200 * 0.88 - 1000
    st.ref = 1100  # 1100*0.88 = 968 < 1000: a loss after tax
    assert resell_opportunity(st, keep=0.88) is None
    st.ref, st.history_points = 1500, 2
    assert resell_opportunity(st, keep=0.88) is None  # no usual price without history


def test_volatile_or_unknown_speed_is_risky():
    st = ItemStats(key="k", name="X", ask=1000, listings=40, ref=1500, ref_basis="t", history_points=48,
                   sales_per_day=None, sales_hours=0, volatility=0.05, pessimistic=1400, reversion=1.0)
    assert resell_opportunity(st, 0.88) is None or resell_opportunity(st, 0.88).risk == RISKY
    st.sales_per_day, st.sales_hours, st.volatility = 30, 24, 0.5
    o = resell_opportunity(st, 0.88)  # big swings are shown, but alone they didn't lose money in simulations
    assert o.risk != RISKY and any("swings" in r for r in o.reasons)
    st.volatility = 0.05
    assert resell_opportunity(st, 0.88).risk != SAFE  # Safe needs proof the price isn't falling
    st.trend_short = 0.0
    assert resell_opportunity(st, 0.88).risk == SAFE


def test_falling_market_resell_targets_recent_price_and_is_risky():
    st = ItemStats(key="k", name="Ore", ask=1000, listings=40, ref=1600, ref_basis="t", history_points=48,
                   sales_per_day=30, sales_hours=24, volatility=0.05, pessimistic=1500,
                   recent_ref=1250, trend_short=-0.06, reversion=1.0)
    o = resell_opportunity(st, 0.88)
    assert abs(o.sell_price - 1250 * 0.94) < 1e-6  # last day's price, falling one more day; not the 3-day median
    assert o.risk == RISKY and any("falling" in r for r in o.reasons)
    st.trend_short = -0.3                  # steep fall: no profit left at tomorrow's price
    assert resell_opportunity(st, 0.88) is None


def test_dips_count_only_as_far_as_history_says_they_recover():
    st = ItemStats(key="k", name="Ore", ask=1000, listings=40, ref=1400, ref_basis="t", history_points=48,
                   sales_per_day=30, sales_hours=24, volatility=0.05, pessimistic=1300, trend_short=0.0)
    st.reversion = 1.0
    assert resell_opportunity(st, 0.88).sell_price == 1400
    st.reversion = 0.5
    assert resell_opportunity(st, 0.88).sell_price == 1200
    st.reversion = 0.0                     # dips never recovered: nothing to resell
    assert resell_opportunity(st, 0.88) is None


def test_plan_respects_ten_listing_slots_and_play_time():
    ops = [op(f"i{i}", 10_000, 3_000, 30) for i in range(40)]
    p = make_plan(ops, 1e9, 3, play_hours=8)
    assert len(p.lines) <= 10
    assert p.minutes <= 8 * 60
    short_day = make_plan(ops, 1e9, 3, play_hours=1)
    assert short_day.minutes <= 60 and short_day.profit < p.profit


def test_lots_and_listing_time():
    o = op("odyle", 1000, 200, per_day=60)   # stackable: 60/day -> lots of 10, 6 listings/day
    assert o.lot == 10 and abs(o.listings_per_day - 6) < 1e-9
    assert abs(o.minutes_per_day - 30) < 1e-9
    o.stats.section = "weapons"               # gear: one item per listing
    assert o.lot == 1 and o.listings_per_day == 60


def test_learned_rules_thin_margins_and_avoid_warnings_are_risky():
    st = ItemStats(key="k", name="X", ask=1000, listings=40, ref=1110, ref_basis="t", history_points=48,
                   sales_per_day=30, sales_hours=24, volatility=0.05, pessimistic=1100, trend_short=0.0, reversion=1.0)
    o = resell_opportunity(st, 0.95)          # ~5% predicted: such trades lost money in simulations
    assert o.risk == RISKY and any("thin margin" in r for r in o.reasons)


def test_plan_caps_each_item_and_stocks_a_few_days():
    a = op("a", 1000, 400, per_day=1000)       # could absorb far more than the budget
    b = op("b", 1000, 300, per_day=1000)
    p = make_plan([a, b], 1_000_000, horizon=3)
    assert all(l.capital <= 0.15 * 1_000_000 + 1e-6 for l in p.lines)
    small = op("c", 1000, 400, per_day=10)
    p3 = make_plan([small], 1e9, horizon=3)
    assert p3.lines[0].units == 30            # three days of sales at once
    assert make_plan([small], 1e9, horizon=1).lines[0].units == 10


def test_realistic_roi_curve():
    from aion_income.opportunities import realistic_roi
    assert realistic_roi(0.10) < 0.03 and abs(realistic_roi(1.0) - 0.59) < 0.02
    assert realistic_roi(0.5) < 0.5 and realistic_roi(2.0) < 2.0


def test_million_run_rules():
    from aion_income.opportunities import MIN_PRED_ROI
    from aion_income.planner import MAX_SHARE, MAX_STOCK_DAYS
    assert MIN_PRED_ROI == 0.20 and MAX_SHARE == 0.15 and MAX_STOCK_DAYS == 7
    st = ItemStats(key="k", name="X", ask=1000, listings=40, ref=1400, ref_basis="t", history_points=48,
                   sales_per_day=30, sales_hours=24, volatility=0.05, pessimistic=600, trend_short=0.0, reversion=1.0)
    o = resell_opportunity(st, 0.88)       # +23% predicted but loses money at the recent low
    assert o.risk == RISKY and any("loses money" in r for r in o.reasons)
    small = op("c", 1000, 400, per_day=10)
    assert make_plan([small], 1e9, horizon=7).lines[0].units == 70   # a week of sales at once
