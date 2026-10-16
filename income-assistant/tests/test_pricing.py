import time
from datetime import datetime, timezone

from aion_income.pricing import (
    build_stats, depth_price, history_stats, sales_from_buckets, sales_from_snapshots,
)

NOW = time.time()


def iso(ts):
    return datetime.fromtimestamp(ts, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def hourly(prices, now=NOW):
    n = len(prices)
    return [{"ts": iso(now - (n - i) * 3600), "close": p, "low": p, "high": p} for i, p in enumerate(prices)]


def row(price, listings=20, **kw):
    r = {"item_key": 1, "name": "Thing", "price": price, "listings": listings,
         "category": {"name": "Ore", "section_id": "materials"}}
    r.update(kw)
    return r


def test_median_ignores_a_bait_listing():
    pts = hourly([1000, 1100, 10_000_005, 1050, 990, 1020, 1010])
    h = history_stats(pts, NOW)
    assert 990 <= h["ref"] <= 1100


def test_history_needs_enough_points():
    assert history_stats(hourly([1000, 1100]), NOW)["ref"] is None


def test_old_points_are_ignored():
    pts = hourly([999_999] * 5, now=NOW - 200 * 3600) + hourly([100] * 8)
    assert history_stats(pts, NOW)["ref"] == 100


def test_sell_price_never_above_cheapest_listing_or_usual_price():
    st = build_stats(row(1200), hourly=hourly([1000] * 10), now=NOW)
    assert st.sell_price == 1000  # listing above usual: usual
    st = build_stats(row(800), hourly=hourly([1000] * 10), now=NOW)
    assert st.sell_price == 800   # someone is cheaper: you must match them


def test_spike_detected_and_ignored():
    st = build_stats(row(10_000_005, listings=1), hourly=hourly([1000] * 10), now=NOW)
    assert st.spike and st.sell_price == 1000
    assert st.thin and not st.trusted


def test_low_stock_is_not_trusted():
    st = build_stats(row(500, listings=2), hourly=hourly([500] * 10), now=NOW)
    assert st.thin and not st.trusted
    assert "only 2 listing(s)" in st.flags


def test_real_sales_win_over_listing_drops():
    st = build_stats(row(1000, units_sold=2800, sold_avg_price=900), hourly=hourly([1000] * 10), now=NOW)
    assert st.sales_per_day == 100
    assert st.ref <= 900 * 1.05  # never assume more than buyers actually paid


def test_sales_from_buckets_counts_drops_damped():
    pts = []
    listings = 50
    for i in range(25):
        pts.append({"ts": iso(NOW - (25 - i) * 600), "listings": listings})
        listings -= 1  # one listing bought every 10 minutes = 144/day
    rate, hours = sales_from_buckets(pts)
    assert hours >= 3.9
    assert 90 < rate < 110  # 144/day * 0.7 damping for cancellations


def test_sales_from_buckets_ignores_new_listings():
    pts = [{"ts": iso(NOW - (25 - i) * 600), "listings": 10 + i} for i in range(25)]
    rate, _ = sales_from_buckets(pts)
    assert rate == 0


def test_sales_from_snapshots_skips_long_gaps():
    s = [(0, 100, 10), (3600, 100, 8), (2 * 3600, 100, 7), (3 * 3600, 100, 7), (30 * 3600, 100, 1)]
    rate, hours = sales_from_snapshots(s)
    assert hours == 3
    assert abs(rate - 3 * 0.7 / 3 * 24) < 1e-6


def test_stale_item_not_trusted():
    s = [(NOW - (20 - i) * 3600, 100, 15) for i in range(20)]
    st = build_stats(row(100, listings=15), snapshots=s, now=NOW)
    assert st.stale and not st.trusted


def test_depth_price_prefers_whole_stacks():
    d = {"units": 50, "points": [{"status": "ok", "cost": 5000, "unit_avg": 100, "units_stacks": 60, "cost_stacks": 6600}]}
    assert depth_price(d) == (110, 60)
    assert depth_price({"units": 50, "points": [{"status": "error"}]}) == (None, None)
    assert depth_price(None) == (None, None)


def test_missing_fields_do_not_crash():
    st = build_stats({"item_key": 5})
    assert st.ask is None and st.sell_price is None and not st.trusted
