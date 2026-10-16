"""Local history: every refresh is saved so the app builds its own transaction history.

aion2.exchange does not publish real sales for most servers (Lumiel included at
the time of writing), so sales are inferred: when the number of listings drops
between two snapshots, those listings were bought (or cancelled; the estimate is
treated as an upper bound and damped in pricing.py).
"""
from __future__ import annotations

import json
import sqlite3
import statistics
import threading
import time
from datetime import datetime

KEEP_DAYS = 60


def parse_ts(value) -> float | None:
    """ISO timestamp (or epoch seconds/ms) -> epoch seconds."""
    if value is None:
        return None
    if isinstance(value, (int, float)):
        return float(value) / (1000.0 if value > 1e11 else 1.0)
    try:
        s = str(value).replace("Z", "+00:00")
        return datetime.fromisoformat(s).timestamp()
    except ValueError:
        return None


class HistoryStore:
    def __init__(self, path: str):
        self.path = path
        self._lock = threading.Lock()
        self.db = sqlite3.connect(path, check_same_thread=False)
        self.db.executescript(
            """
            CREATE TABLE IF NOT EXISTS snapshots (
                server INTEGER NOT NULL, item TEXT NOT NULL, ts REAL NOT NULL,
                price REAL, listings INTEGER, units_sold INTEGER, sold_avg REAL,
                PRIMARY KEY (server, item, ts)
            );
            CREATE TABLE IF NOT EXISTS cache (
                server INTEGER NOT NULL, item TEXT NOT NULL, kind TEXT NOT NULL,
                fetched REAL NOT NULL, body TEXT NOT NULL,
                PRIMARY KEY (server, item, kind)
            );
            CREATE TABLE IF NOT EXISTS market (
                server INTEGER PRIMARY KEY, fetched REAL NOT NULL, body TEXT NOT NULL
            );
            """
        )
        self.db.commit()

    def close(self):
        with self._lock:
            self.db.close()

    # ── snapshots ────────────────────────────────────────────────────────

    def add_snapshot(self, server: int, rows: list[dict], now: float | None = None) -> int:
        now = now or time.time()
        data = []
        for r in rows:
            key = r.get("item_key")
            if key is None:
                continue
            ts = parse_ts(r.get("captured_at")) or now
            data.append((server, str(key), ts, r.get("price"), r.get("listings"), r.get("units_sold"),
                         r.get("sold_avg_price")))
        with self._lock:
            cur = self.db.executemany("INSERT OR IGNORE INTO snapshots VALUES (?,?,?,?,?,?,?)", data)
            self.db.execute("DELETE FROM snapshots WHERE ts < ?", (now - KEEP_DAYS * 86400,))
            self.db.commit()
            return cur.rowcount

    def series(self, server: int, item: str, since: float) -> list[tuple]:
        """[(ts, price, listings)] oldest first."""
        with self._lock:
            return self.db.execute(
                "SELECT ts, price, listings FROM snapshots WHERE server=? AND item=? AND ts>=? ORDER BY ts",
                (server, str(item), since),
            ).fetchall()

    def counter_span(self, server: int, since: float, min_items: int = 50):
        """(t0, total0, t1, total1): the site's "units sold" totals at the start and end of your
        history since `since`, over the items seen at least 6 hours apart (each item has its own
        capture time, so times are the medians). None with too little history."""
        first: dict[str, tuple] = {}
        last: dict[str, tuple] = {}
        with self._lock:
            for item, ts, units in self.db.execute(
                    "SELECT item, ts, units_sold FROM snapshots WHERE server=? AND ts>=? AND units_sold IS NOT NULL "
                    "ORDER BY ts", (server, since)):
                first.setdefault(item, (ts, units))
                last[item] = (ts, units)
        pairs = [(first[k], last[k]) for k in first if last[k][0] - first[k][0] >= 6 * 3600]
        if len(pairs) < min_items:
            return None
        t0 = statistics.median(a[0] for a, _ in pairs)
        t1 = statistics.median(b[0] for _, b in pairs)
        return t0, float(sum(a[1] for a, _ in pairs)), t1, float(sum(b[1] for _, b in pairs))

    def all_series(self, server: int, since: float) -> dict[str, list[tuple]]:
        out: dict[str, list[tuple]] = {}
        with self._lock:
            for item, ts, price, listings in self.db.execute(
                "SELECT item, ts, price, listings FROM snapshots WHERE server=? AND ts>=? ORDER BY ts",
                (server, since),
            ):
                out.setdefault(item, []).append((ts, price, listings))
        return out

    def snapshot_count(self, server: int) -> int:
        with self._lock:
            return self.db.execute("SELECT COUNT(DISTINCT ts) FROM snapshots WHERE server=?", (server,)).fetchone()[0]

    # ── response cache ───────────────────────────────────────────────────

    def cache_get(self, server: int, item: str, kind: str, max_age: float):
        with self._lock:
            row = self.db.execute("SELECT fetched, body FROM cache WHERE server=? AND item=? AND kind=?",
                                  (server, str(item), kind)).fetchone()
        if not row or time.time() - row[0] > max_age:
            return None
        return json.loads(row[1])

    def cache_put(self, server: int, item: str, kind: str, body) -> None:
        with self._lock:
            self.db.execute("INSERT OR REPLACE INTO cache VALUES (?,?,?,?,?)",
                            (server, str(item), kind, time.time(), json.dumps(body)))
            self.db.commit()

    def save_market(self, server: int, rows: list[dict]) -> None:
        with self._lock:
            self.db.execute("INSERT OR REPLACE INTO market VALUES (?,?,?)", (server, time.time(), json.dumps(rows)))
            self.db.commit()

    def load_market(self, server: int):
        """(fetched_epoch, rows) of the last good market download, or None."""
        with self._lock:
            row = self.db.execute("SELECT fetched, body FROM market WHERE server=?", (server,)).fetchone()
        return (row[0], json.loads(row[1])) if row else None
