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
    fast = op("fast", 100_000, 8_000, per_day=20)     # 8% but sells 20/day
    slow = op("slow", 100_000, 30_000, per_day=0.2)   # 30% but one every 5 days
    p = make_plan([slow, fast], 2_000_000, 3)
    assert p.lines[0].op.name == "fast"
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
    ops = [op(f"i{i}", 5_000_000, 600_000, 50) for i in range(50)]
    assert max(t.amount for t in holding_tiers(ops, 3)) <= HOLD_CAP


def test_no_opportunities():
    assert holding_tiers([], 3) == []
    assert make_plan([], 1e6).lines == []


def test_resell_needs_history_and_margin_after_tax():
    st = ItemStats(key="k", name="Odyle", ask=1000, listings=40, ref=1200, ref_basis="t", history_points=48,
                   sales_per_day=30, sales_hours=24, volatility=0.05, pessimistic=1150)
    o = resell_opportunity(st, keep=0.88)
    assert o and o.profit_unit == 1200 * 0.88 - 1000
    st.ref = 1100  # 1100*0.88 = 968 < 1000: a loss after tax
    assert resell_opportunity(st, keep=0.88) is None
    st.ref, st.history_points = 1500, 2
    assert resell_opportunity(st, keep=0.88) is None  # no usual price without history


def test_volatile_or_unknown_speed_is_risky():
    st = ItemStats(key="k", name="X", ask=1000, listings=40, ref=1500, ref_basis="t", history_points=48,
                   sales_per_day=None, sales_hours=0, volatility=0.05, pessimistic=1400)
    assert resell_opportunity(st, 0.88) is None or resell_opportunity(st, 0.88).risk == RISKY
    st.sales_per_day, st.sales_hours, st.volatility = 30, 24, 0.5
    assert resell_opportunity(st, 0.88).risk == RISKY
    st.volatility = 0.05
    assert resell_opportunity(st, 0.88).risk == SAFE
