"""Turn raw market rows and history into prices you can rely on.

Rules (from how aion2.exchange data behaves):
  * `price` on a row is the CHEAPEST LISTING right now, not a sale price.
  * Single listings can be bait (10,000,005 for an item that sold for 1,111),
    so the typical price comes from the median of the hourly history, which one
    odd listing cannot move.
  * Items with very few listings, or that sit on the market without selling,
    are never used as evidence for a profit.
"""
from __future__ import annotations

import statistics
import time
from dataclasses import dataclass, field

from .store import parse_ts

MIN_LISTINGS = 3            # below this a listing says nothing about value
SPIKE_RATIO = 1.3           # ask this far above the typical price = not a real price
CANCEL_DAMPING = 0.7        # share of vanished listings assumed to be real purchases
HISTORY_HOURS = 72          # window for the typical price
MIN_HISTORY_POINTS = 6      # fewer hourly points than this and the median is not trusted


@dataclass
class ItemStats:
    key: str
    name: str
    section: str | None = None
    category: str | None = None
    ask: float | None = None
    listings: int = 0
    change_pct: float | None = None
    units_sold_28d: int = 0
    sold_avg: float | None = None
    ref: float | None = None          # typical price
    ref_basis: str = "none"
    pessimistic: float | None = None  # what you get if the market dips (25th percentile)
    volatility: float | None = None   # coefficient of variation of hourly prices
    trend_7d: float | None = None
    supply_kind: str | None = None    # "Daily"/"Weekly" when a supply request asks for this item
    supply_hours_left: float | None = None
    reversion: float = 0.3            # share of a dip that recovers within a day (learned, see calibrate)
    drift: float = 1.0                # typical day-over-day price change across the market (learned)
    recent_ref: float | None = None   # median of the last 24 hours
    trend_short: float | None = None  # last 24 h vs the 2 days before (negative = falling)
    history_points: int = 0
    sales_per_day: float | None = None
    sales_basis: str = "unknown"
    sales_hours: float = 0.0          # how many hours of evidence the sales rate stands on
    bulk_unit: float | None = None    # average unit price when buying `bulk_units` now
    bulk_units: int | None = None
    flags: list = field(default_factory=list)

    @property
    def thin(self) -> bool:
        return self.listings < MIN_LISTINGS and self.units_sold_28d <= 0

    @property
    def spike(self) -> bool:
        return bool(self.ask and self.ref and self.ask > self.ref * SPIKE_RATIO)

    @property
    def stale(self) -> bool:
        """Listed, but nothing sold across a meaningful stretch of evidence."""
        return self.listings > 0 and self.sales_per_day == 0 and self.sales_hours >= 12

    @property
    def falling(self) -> bool:
        return self.trend_short is not None and self.trend_short < -0.05

    @property
    def target(self) -> float | None:
        """Price to aim for when listing later: the usual price, but never above the last
        24 hours (in a falling market the 3-day median lags behind)."""
        cands = [x for x in (self.ref, self.recent_ref) if x]
        return min(cands) if cands else None

    @property
    def sell_price(self) -> float | None:
        """What you can realistically list at and still sell: never above the typical
        price, never above the last day's prices, never above the cheapest competing listing."""
        cands = [x for x in (self.ask, self.ref, self.recent_ref) if x]
        if not cands:
            return None
        if self.spike:
            return self.target
        return min(cands)

    def _next_day(self, price):
        """A falling price usually keeps falling for a while: expect one more day of the
        same drop by the time it sells (never assume a rebound)."""
        if price is None:
            return None
        if self.trend_short is not None and self.trend_short < 0:
            return price * max(0.5, 1 + self.trend_short)
        return price

    @property
    def expected_sell(self) -> float | None:
        """What a listing made now is expected to sell for: a falling trend continues and the
        whole market drifts the way it has been drifting (never assumed to rise)."""
        v = self._next_day(self.sell_price)
        if v is None:
            return None
        if self.supply_kind and (self.supply_hours_left or 0) > 24:
            return v  # a running supply request holds demand up: no market drift
        return v * min(1.0, self.drift)

    @property
    def expected_target(self) -> float | None:
        """Resale price for something bought below its usual price: only the share of such
        dips that history shows actually recovers within a day."""
        t = self._next_day(self.target)
        if t is None:
            return None
        if self.ask and self.ask < t:
            t = self.ask + self.reversion * (t - self.ask)
        return t * min(1.0, self.drift)

    @property
    def sell_pessimistic(self) -> float | None:
        s = self.expected_sell
        if s is None:
            return None
        if self.pessimistic:
            return min(s, self.pessimistic)
        return s * 0.85

    @property
    def buy_price(self) -> float | None:
        return self.ask

    @property
    def trusted(self) -> bool:
        """Good enough to base a profit on."""
        return not self.thin and not self.stale and self.sell_price is not None

    @property
    def days_per_unit(self) -> float | None:
        if not self.sales_per_day:
            return None
        return 1.0 / self.sales_per_day


def _num(v):
    return float(v) if isinstance(v, (int, float)) and v == v else None


def percentile(values, q):
    v = sorted(values)
    if not v:
        return None
    idx = (len(v) - 1) * q
    lo, hi = int(idx), min(int(idx) + 1, len(v) - 1)
    return v[lo] + (v[hi] - v[lo]) * (idx - lo)


def _points_ts(points):
    out = []
    for p in points or []:
        ts = parse_ts(p.get("ts") or p.get("t") or p.get("time") or p.get("bucket"))
        if ts is not None:
            out.append((ts, p))
    out.sort(key=lambda x: x[0])
    return out


def recent_split(series, now: float):
    """(median of the last 24 h, change vs the 24-72 h before) from [(ts, price)]."""
    last = [p for t, p in series if now - 24 * 3600 <= t <= now and p]
    older = [p for t, p in series if now - 72 * 3600 <= t < now - 24 * 3600 and p]
    recent = statistics.median(last) if len(last) >= 2 else None
    trend = None
    if recent and len(older) >= 2:
        base = statistics.median(older)
        trend = recent / base - 1 if base else None
    return recent, trend


DEFAULT_REVERSION = 0.3
MIN_CALIBRATION_SAMPLES = 30


def calibrate(series_by_item: dict) -> tuple[float, float, int]:
    """Learn from price history how markets really behave a day later.

    series_by_item: item -> [(ts, price)] oldest first (cheapest listing over time).
    Returns (reversion, drift, samples):
      reversion - when the cheapest listing sat 10%+ below its 3-day median, the median share
                  of that gap that had closed ~24 h later (0 = dips never recover, 1 = fully)
      drift     - median day-over-day change of prices across items
    With too little history it returns cautious defaults.
    """
    closes, ratios = [], []
    for series in series_by_item.values():
        pts = [(t, p) for t, p in series if p]
        if len(pts) < 6:
            continue
        j = 0
        for i, (t, p) in enumerate(pts):
            while j < len(pts) and pts[j][0] < t + 18 * 3600:
                j += 1
            if j >= len(pts) or pts[j][0] > t + 30 * 3600:
                continue
            nxt = pts[j][1]
            ratios.append(nxt / p)
            window = [q for tt, q in pts[: i + 1] if tt >= t - 72 * 3600]
            if len(window) >= 4:
                ref = statistics.median(window)
                if p < 0.9 * ref:
                    closes.append((nxt - p) / (ref - p))
    drift = statistics.median(ratios) if len(ratios) >= MIN_CALIBRATION_SAMPLES else 1.0
    if len(closes) >= MIN_CALIBRATION_SAMPLES:
        reversion = max(0.0, min(1.0, statistics.median(closes)))
    else:
        reversion = DEFAULT_REVERSION
    return reversion, drift, len(closes)


def history_stats(points, now: float | None = None):
    """Typical price, pessimistic price, volatility and 7-day trend from hourly points."""
    now = now or time.time()
    pts = _points_ts(points)
    def close(p):
        return _num(p.get("close")) or _num(p.get("low")) or _num(p.get("price"))
    recent = [(ts, close(p), p) for ts, p in pts if ts >= now - HISTORY_HOURS * 3600 and close(p)]
    res = {"ref": None, "pessimistic": None, "volatility": None, "trend_7d": None, "n": len(recent)}
    res["recent"], res["trend_short"] = recent_split([(ts, c) for ts, c, _ in recent], now)
    if len(recent) >= MIN_HISTORY_POINTS:
        closes = [c for _, c, _ in recent]
        res["ref"] = statistics.median(closes)
        lows = [(_num(p.get("low")) or c) for _, c, p in recent]
        res["pessimistic"] = percentile(lows, 0.25)
        mean = statistics.fmean(closes)
        res["volatility"] = statistics.pstdev(closes) / mean if mean > 0 else None
    week = [(ts, close(p)) for ts, p in pts if ts >= now - 7 * 86400 and close(p)]
    if len(week) >= 2 and week[-1][0] - week[0][0] >= 48 * 3600:
        first = statistics.median(c for _, c in week[: max(1, len(week) // 10)])
        last = statistics.median(c for _, c in week[-max(1, len(week) // 10):])
        res["trend_7d"] = last / first - 1 if first else None
    return res


def sales_from_buckets(points):
    """(listings cleared per day, hours covered) from 10-minute buckets with listing counts."""
    pts = _points_ts(points)
    if len(pts) < 2:
        return None, 0.0
    gone = 0.0
    seen = False
    for (_, a), (_, b) in zip(pts, pts[1:]):
        la = a.get("listings_max", a.get("listings"))
        lb = b.get("listings_min", b.get("listings"))
        if la is None or lb is None:
            continue
        seen = True
        if lb < la:
            gone += la - lb
    if not seen:
        return None, 0.0
    hours = max((pts[-1][0] - pts[0][0]) / 3600.0, len(pts) * 10 / 60.0 - 10 / 60.0)
    if hours < 2:
        return None, hours
    return gone * CANCEL_DAMPING / hours * 24.0, hours


def sales_from_snapshots(series, max_gap_h: float = 3.0):
    """Same idea on the app's own refresh history: [(ts, price, listings)]."""
    gone, hours = 0.0, 0.0
    for (t0, _, l0), (t1, _, l1) in zip(series, series[1:]):
        gap = (t1 - t0) / 3600.0
        if gap <= 0 or gap > max_gap_h or l0 is None or l1 is None:
            continue
        hours += gap
        if l1 < l0:
            gone += l0 - l1
    if hours < 3:
        return None, hours
    return gone * CANCEL_DAMPING / hours * 24.0, hours


def depth_price(depth):
    """(average unit price, units) for buying a batch right now, from /depth."""
    if not depth:
        return None, None
    pts = [p for p in depth.get("points") or [] if p.get("status", "ok") == "ok"]
    if not pts:
        return None, None
    p = pts[-1]
    units = depth.get("units")
    if _num(p.get("cost_stacks")) and _num(p.get("units_stacks")):
        return p["cost_stacks"] / p["units_stacks"], int(p["units_stacks"])
    if _num(p.get("unit_avg")):
        return float(p["unit_avg"]), int(units) if units else None
    if _num(p.get("cost")) and units:
        return p["cost"] / units, int(units)
    return None, None


def build_stats(row: dict, hourly=None, buckets=None, snapshots=None, depth=None, now=None) -> ItemStats:
    cat = row.get("category") or {}
    st = ItemStats(
        key=str(row.get("item_key")),
        name=row.get("name") or str(row.get("item_key")),
        section=cat.get("section_id") if isinstance(cat, dict) else None,
        category=cat.get("name") if isinstance(cat, dict) else None,
        ask=_num(row.get("price")) if (_num(row.get("price")) or 0) > 0 else None,
        listings=int(row.get("listings") or 0),
        change_pct=_num(row.get("change_pct")),
        units_sold_28d=int(row.get("units_sold") or 0),
        sold_avg=_num(row.get("sold_avg_price")),
    )
    # typical price: site history, else our own snapshots, else published sales
    if hourly:
        h = history_stats(hourly, now)
        st.history_points = h["n"]
        st.trend_7d = h["trend_7d"]
        st.recent_ref, st.trend_short = h["recent"], h["trend_short"]
        if h["ref"]:
            st.ref, st.ref_basis = h["ref"], "site history (72 h median)"
            st.pessimistic, st.volatility = h["pessimistic"], h["volatility"]
    if snapshots:
        latest = max(t for t, _, _ in snapshots)
        window = [(t, p) for t, p, _ in snapshots if p and t >= latest - HISTORY_HOURS * 3600]
        prices = [p for _, p in window]
        if st.ref is None and len(prices) >= MIN_HISTORY_POINTS:
            st.ref, st.ref_basis = statistics.median(prices), "your refresh history (72 h median)"
            st.pessimistic = percentile(prices, 0.25)
            mean = statistics.fmean(prices)
            st.volatility = statistics.pstdev(prices) / mean if mean else None
            st.history_points = len(prices)
        if st.recent_ref is None:
            st.recent_ref, st.trend_short = recent_split(window, latest)
        if st.trend_7d is None:
            week = sorted((t, p) for t, p, _ in snapshots if p and t >= latest - 7 * 86400)
            if len(week) >= 4 and week[-1][0] - week[0][0] >= 48 * 3600:
                n = max(1, len(week) // 5)
                first = statistics.median(p for _, p in week[:n])
                last = statistics.median(p for _, p in week[-n:])
                st.trend_7d = last / first - 1 if first else None
    if st.ref is None and st.sold_avg and st.units_sold_28d > 0:
        st.ref, st.ref_basis = st.sold_avg, "market sales average (28 days)"
    if st.sold_avg and st.units_sold_28d > 0 and st.ref:
        # real sales are the best evidence there is: never assume more than buyers paid
        st.ref = min(st.ref, st.sold_avg * 1.05)

    # how fast it sells
    if st.units_sold_28d > 0:
        st.sales_per_day, st.sales_basis, st.sales_hours = st.units_sold_28d / 28.0, "market sales (28 days)", 28 * 24
    else:
        best = None
        for fn, src, basis in ((sales_from_buckets, buckets, "listings sold on the site (last hours)"),
                               (sales_from_snapshots, snapshots, "listings sold between your refreshes")):
            if not src:
                continue
            rate, hours = fn(src)
            if rate is not None and (best is None or hours > best[2]):
                best = (rate, basis, hours)
        if best:
            st.sales_per_day, st.sales_basis, st.sales_hours = best

    st.bulk_unit, st.bulk_units = depth_price(depth)

    if st.thin:
        st.flags.append(f"only {st.listings} listing(s)")
    if st.spike:
        st.flags.append("cheapest listing far above its usual price")
    if st.stale:
        st.flags.append("listed but not selling")
    if st.volatility and st.volatility > 0.35:
        st.flags.append("price swings a lot")
    if st.falling:
        st.flags.append(f"price falling ({st.trend_short:+.0%} in the last day)")
    return st
