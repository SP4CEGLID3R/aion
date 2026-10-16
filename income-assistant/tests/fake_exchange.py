"""A local stand-in for aion2.exchange used by the tests.

Market rows are real (see fixtures/market_rows.json); price history, 10-minute
buckets and depth are generated deterministically around each item's real price
in the same shapes the real API returns.
"""
from __future__ import annotations

import json
import os
import random
import threading
import time
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

FIX = os.path.join(os.path.dirname(__file__), "fixtures")
LUMIEL = 12104


def load_rows():
    with open(os.path.join(FIX, "market_rows.json"), encoding="utf-8") as f:
        return json.load(f)["rows"]


def load_servers():
    with open(os.path.join(FIX, "servers.json"), encoding="utf-8") as f:
        return json.load(f)


def iso(ts: float) -> str:
    return datetime.fromtimestamp(ts, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


class FakeExchange:
    def __init__(self):
        self.rows = load_rows()
        self.by_key = {str(r["item_key"]): r for r in self.rows}
        self.servers = load_servers()
        self.now = time.time()
        self.fail = set()        # endpoint names that answer 500
        self.requests = []
        self.dips = {}           # item key -> ask multiplier (simulate a cheap listing)
        self.sales_per_day = {}  # item key -> listings cleared per day in buckets
        self._server = None

    # ── synthetic history ────────────────────────────────────────────────
    def base_price(self, key):
        r = self.by_key.get(str(key))
        return float(r["price"]) if r and r.get("price") else None

    def hourly(self, key):
        p = self.base_price(key)
        if not p:
            return []
        rnd = random.Random(int(key))
        pts = []
        for h in range(168, 0, -1):
            ts = self.now - h * 3600
            c = p * (1 + rnd.uniform(-0.06, 0.06))
            pts.append({"ts": iso(ts), "open": c, "close": round(c), "low": round(c * 0.97), "high": round(c * 1.03)})
        return pts

    def buckets(self, key):
        r = self.by_key.get(str(key))
        if not r or not r.get("price"):
            return []
        rate = self.sales_per_day.get(str(key), max(1.0, (r.get("listings") or 0) / 4))
        n = 24  # the real API returns about 4 hours of 10-minute buckets
        listings = (r.get("listings") or 0) + int(rate * n * 10 / 1440) + 1
        pts, acc = [], 0.0
        for i in range(n):
            ts = self.now - (n - i) * 600
            acc += rate * 10 / 1440
            drop = int(acc)
            acc -= drop
            listings = max(0, listings - drop)
            pts.append({"ts": iso(ts), "low": r["price"], "high": r["price"] * 1.05,
                        "listings": listings, "listings_min": listings, "listings_max": listings + drop})
        return pts

    def depth(self, key):
        p = self.current_price(key)
        if not p:
            return {"item_key": int(key), "units": 50, "points": []}
        avg = p * 1.04
        return {"item_key": int(key), "units": 50,
                "points": [{"status": "ok", "cost": avg * 50, "unit_avg": avg, "units_stacks": 50, "cost_stacks": avg * 50}]}

    def current_price(self, key):
        p = self.base_price(key)
        if p and str(key) in self.dips:
            p = round(p * self.dips[str(key)])
        return p

    def market_rows(self):
        out = []
        for r in self.rows:
            r2 = dict(r)
            if str(r["item_key"]) in self.dips and r2.get("price"):
                r2["price"] = self.current_price(r["item_key"])
            out.append(r2)
        return out

    # ── HTTP ─────────────────────────────────────────────────────────────
    def handle(self, path, q):
        name = path.rstrip("/").split("/")[-1]
        if name in self.fail or (path.startswith("/api/items/") and "details" in self.fail):
            return 500, {"detail": "error code 7513"}
        if path == "/api/servers":
            return 200, self.servers
        if path == "/api/items":
            lim = int(q.get("limit", ["50"])[0])
            off = int(q.get("offset", ["0"])[0])
            if lim > 200:
                return 422, {"detail": "limit must be <= 200"}
            if int(q.get("server", ["0"])[0]) != LUMIEL:
                return 200, {"total": 0, "limit": lim, "offset": off, "items": []}
            rows = self.market_rows()
            return 200, {"total": len(rows), "limit": lim, "offset": off, "items": rows[off:off + lim]}
        parts = path.split("/")
        if len(parts) == 5 and parts[2] == "items":
            key, kind = parts[3], parts[4]
            if kind == "prices":
                if q.get("bucket", [None])[0] == "10m":
                    return 200, {"points": self.buckets(key), "summary": {}}
                return 200, {"points": self.hourly(key)}
            if kind == "depth":
                return 200, self.depth(key)
        return 404, {"detail": "not found"}

    def start(self):
        fake = self

        class H(BaseHTTPRequestHandler):
            def do_GET(self):
                u = urlparse(self.path)
                fake.requests.append(self.path)
                code, body = fake.handle(u.path, parse_qs(u.query))
                data = json.dumps(body).encode()
                self.send_response(code)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

            def log_message(self, *a):
                pass

        self._server = ThreadingHTTPServer(("127.0.0.1", 0), H)
        threading.Thread(target=self._server.serve_forever, daemon=True).start()
        return f"http://127.0.0.1:{self._server.server_address[1]}/api"

    def stop(self):
        if self._server:
            self._server.shutdown()
            self._server.server_close()
