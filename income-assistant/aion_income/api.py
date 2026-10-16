"""Client for the public aion2.exchange API (read-only web requests, no game access).

Endpoints used (open, no token):
  GET /api/servers                          servers with ids, region, faction
  GET /api/items?server=&limit=&offset=     every item on a server's market (cheapest ask,
                                            listings, change, and 28-day sales where published)
  GET /api/items/{key}/prices?server=       hourly price history
  GET /api/items/{key}/prices?server=&from=&bucket=10m
                                            10-minute buckets incl. listing counts
  GET /api/items/{key}/depth?server=        what buying a batch costs right now

The site is run by one person, so requests are rate limited and retried gently.
"""
from __future__ import annotations

import threading
import time
from datetime import datetime, timedelta, timezone

import requests

BASE_URL = "https://aion2.exchange/api"
USER_AGENT = "Aion2IncomeAssistant/1.0 (personal market helper)"
PAGE = 200  # the API rejects limit > 200


class ApiError(Exception):
    pass


class ExchangeClient:
    def __init__(self, base_url: str = BASE_URL, session: requests.Session | None = None,
                 min_interval: float = 0.15, timeout: float = 25.0, retry_waits=(2.0, 6.0)):
        self.base = base_url.rstrip("/")
        self.session = session or requests.Session()
        self.session.headers.update({"User-Agent": USER_AGENT, "Accept": "application/json"})
        self.min_interval = min_interval
        self.timeout = timeout
        self.retry_waits = retry_waits
        self._lock = threading.Lock()
        self._next_slot = 0.0
        self.request_count = 0

    def _wait_turn(self):
        with self._lock:
            now = time.monotonic()
            slot = max(now, self._next_slot)
            self._next_slot = slot + self.min_interval
        if slot > now:
            time.sleep(slot - now)

    def get_json(self, path: str, params: dict | None = None):
        url = self.base + path
        last = None
        for attempt in range(len(self.retry_waits) + 1):
            self._wait_turn()
            self.request_count += 1
            try:
                r = self.session.get(url, params=params, timeout=self.timeout)
            except requests.RequestException as e:
                last = ApiError(f"{type(e).__name__}: {e}")
            else:
                if r.status_code == 200:
                    try:
                        return r.json()
                    except ValueError:
                        last = ApiError(f"invalid JSON from {url}")
                else:
                    last = ApiError(f"HTTP {r.status_code} from {url}")
                    if r.status_code != 429 and r.status_code < 500:
                        raise last  # a 4xx other than rate limiting will not get better
            if attempt < len(self.retry_waits):
                time.sleep(self.retry_waits[attempt])
        raise last or ApiError(f"no response from {url}")

    # ── endpoints ────────────────────────────────────────────────────────

    def servers(self) -> list[dict]:
        j = self.get_json("/servers")
        if not isinstance(j, dict) or not isinstance(j.get("servers"), list):
            raise ApiError("aion2.exchange /servers returned no server list")
        return j["servers"]

    def find_server(self, name: str = "Lumiel") -> dict:
        matches = [s for s in self.servers() if str(s.get("name", "")).strip().lower() == name.lower()]
        if not matches:
            raise ApiError(f"server '{name}' not found on aion2.exchange")
        matches.sort(key=lambda s: (not s.get("has_data", True),))
        return matches[0]

    def items(self, server_id: int, window: str = "24h") -> list[dict]:
        """Every market row for a server. Raises if the listing comes back incomplete."""
        for page in (PAGE, 50):
            rows, offset, total = [], 0, None
            ok = True
            while True:
                try:
                    j = self.get_json("/items", {"server": server_id, "limit": page, "offset": offset,
                                                 "window": window, "sort": "name", "order": "asc"})
                except ApiError:
                    ok = False
                    break
                batch = j.get("items") if isinstance(j, dict) else None
                if not isinstance(batch, list):
                    ok = False
                    break
                if total is None:
                    total = int(j.get("total") or 0)
                rows.extend(batch)
                offset += page
                if len(batch) < page or offset >= total:
                    break
            if ok and rows and (not total or len(rows) >= total):
                return rows
        raise ApiError("could not download the full market list from aion2.exchange")

    def price_history(self, item_key, server_id: int) -> list[dict]:
        j = self.get_json(f"/items/{item_key}/prices", {"server": server_id})
        pts = j.get("points") if isinstance(j, dict) else None
        return pts if isinstance(pts, list) else []

    def recent_buckets(self, item_key, server_id: int, hours: int = 24) -> list[dict]:
        since = (datetime.now(timezone.utc) - timedelta(hours=hours)).strftime("%Y-%m-%dT%H:%M:%SZ")
        j = self.get_json(f"/items/{item_key}/prices", {"server": server_id, "from": since, "bucket": "10m"})
        pts = j.get("points") if isinstance(j, dict) else None
        return pts if isinstance(pts, list) else []

    def depth(self, item_key, server_id: int) -> dict:
        j = self.get_json(f"/items/{item_key}/depth", {"server": server_id})
        if not isinstance(j, dict):
            return {"units": None, "points": []}
        pts = j.get("points")
        return {"units": j.get("units"), "points": pts if isinstance(pts, list) else []}
