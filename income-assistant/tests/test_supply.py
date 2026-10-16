from aion_income import supply
from aion_income.pricing import ItemStats


def test_request_expiry_and_roundtrip():
    r = supply.new_request("1", "Odyle", "Daily", 5, now=1000.0)
    assert r.expires == 1000.0 + 86400 and r.active(1000.0) and not r.active(1000.0 + 86401)
    w = supply.new_request("2", "Ore", "Weekly", now=0.0, hours_left=10)
    assert w.hours_left(0.0) == 10
    back = supply.from_json(supply.to_json([r, w]))
    assert [x.key for x in back] == ["1", "2"] and back[0].qty == 5
    assert supply.from_json([{"bad": 1}, "x", {"key": 1, "name": "a", "kind": "Monthly", "added": 0, "expires": 1}]) == []


def test_apply_boosts_demand_only_while_active():
    st = ItemStats(key="1", name="Odyle", ask=100, listings=20, sales_per_day=10)
    reqs = [supply.new_request("1", "Odyle", "Weekly", now=0.0)]
    assert supply.apply({"1": st}, reqs, now=3600.0) == 1
    assert st.sales_per_day == 10 * supply.DEMAND_BOOST and st.supply_kind == "Weekly"
    st2 = ItemStats(key="1", name="Odyle", ask=100, listings=20, sales_per_day=10)
    assert supply.apply({"1": st2}, reqs, now=8 * 86400.0) == 0 and st2.sales_per_day == 10


def test_weekly_tip_follows_the_reset_cycle():
    import datetime
    base = datetime.datetime(2026, 10, 12, 12, 0)  # a Monday
    tips = [supply.weekly_tip((base + datetime.timedelta(days=d)).timestamp()) for d in range(7)]
    assert "in 2 days" in tips[0] and "in 1 day" in tips[1]
    assert "reset day" in tips[2] and "after the weekly reset" in tips[3]
    assert all(t for t in tips)
