"""Supply requests: daily (Emergency) and weekly requests that ask players to hand in items.

While a request is up, more players buy the items it asks for, so those items sell
faster and their price holds up better; when it ends, that extra demand stops.
There is no public feed of which requests are active, and the app never reads the
game, so you type in what your request board shows.
"""
from __future__ import annotations

import time
from dataclasses import asdict, dataclass

KINDS = {"Daily": 24 * 3600, "Weekly": 7 * 24 * 3600}
DEMAND_BOOST = 1.3        # extra buyers while a request is up (assumed; no public data to measure it)
ENDING_SOON_HOURS = 24


@dataclass
class SupplyRequest:
    key: str
    name: str
    kind: str          # "Daily" or "Weekly"
    qty: int           # how many the request asks for (information only)
    added: float
    expires: float

    def hours_left(self, now: float | None = None) -> float:
        return (self.expires - (time.time() if now is None else now)) / 3600.0

    def active(self, now: float | None = None) -> bool:
        return self.hours_left(now) > 0


def new_request(key: str, name: str, kind: str, qty: int = 1, now: float | None = None,
                hours_left: float | None = None) -> SupplyRequest:
    if kind not in KINDS:
        raise ValueError(f"kind must be one of {list(KINDS)}")
    now = time.time() if now is None else now
    expires = now + (hours_left * 3600 if hours_left else KINDS[kind])
    return SupplyRequest(str(key), name, kind, max(1, int(qty)), now, expires)


def to_json(reqs: list[SupplyRequest]) -> list[dict]:
    return [asdict(r) for r in reqs]


def from_json(raw) -> list[SupplyRequest]:
    out = []
    if not isinstance(raw, list):
        return out
    for r in raw:
        try:
            if r["kind"] in KINDS:
                out.append(SupplyRequest(str(r["key"]), str(r["name"]), r["kind"], int(r.get("qty", 1)),
                                         float(r["added"]), float(r["expires"])))
        except (KeyError, TypeError, ValueError):
            continue
    return out


def apply(stats: dict, reqs: list[SupplyRequest], now: float | None = None) -> int:
    """Mark items in active requests and raise their expected sales. Returns how many applied."""
    n = 0
    for r in reqs:
        st = stats.get(r.key)
        if not st or not r.active(now):
            continue
        st.supply_kind = r.kind
        st.supply_hours_left = r.hours_left(now)
        if st.sales_per_day is not None:
            st.sales_per_day *= DEMAND_BOOST
        left = st.supply_hours_left
        when = f"{left:.0f} h" if left < 48 else f"{left / 24:.1f} days"
        st.flags.append(f"in a {r.kind.lower()} supply request ({when} left)")
        n += 1
    return n


WEEKLY_RESET_WEEKDAY = 2  # Wednesday


def weekly_tip(now: float | None = None) -> str:
    """Where we are in the weekly supply-request cycle, and what the price history showed.

    In 10 days of data from five servers (one clean weekly reset), crafted items were
    cheapest just before the reset and sold best the day after; materials rose ~20%
    right after it. One week is a hint, not a law.
    """
    import datetime as _dt
    wd = _dt.datetime.fromtimestamp(time.time() if now is None else now).weekday()
    days_to_reset = (WEEKLY_RESET_WEEKDAY - wd) % 7
    if days_to_reset in (1, 2):
        return ("Weekly reset in %d day%s: crafted items and materials were cheapest just before it. "
                "Good time to buy materials and craft, then sell after the reset." % (days_to_reset, "" if days_to_reset == 1 else "s"))
    if days_to_reset == 0:
        return ("Weekly reset day: new supply requests. Crafted items got busiest the day after, and materials "
                "rose ~20%. Hold crafted stock for tomorrow, sell spare materials.")
    if wd == (WEEKLY_RESET_WEEKDAY + 1) % 7:
        return ("Day after the weekly reset: crafted items sold best on this day, and materials were ~20% pricier. "
                "List crafted stock now.")
    return ("Mid-week: prices drifted down toward the next reset. Sell steadily, and buy materials closer to "
            "the reset.")
