"""One refresh: download the market, save it to history, analyse everything."""
from __future__ import annotations

import concurrent.futures
import json
import os
import time
from dataclasses import dataclass, field

from .api import ApiError, ExchangeClient
from .crafting import CraftContext
from .opportunities import all_opportunities
from .pricing import build_stats
from .recipes import PROFESSIONS, RecipeBook, load_book
from .store import HistoryStore

LUMIEL_FALLBACK = {"server_id": 12104, "name": "Lumiel", "region": "NAE", "faction": "Asmodian"}

HOURLY_TTL = 30 * 60
BUCKETS_TTL = 20 * 60
DEPTH_TTL = 15 * 60
MAX_DETAIL_ITEMS = 90


def default_data_dir() -> str:
    base = os.environ.get("APPDATA") or os.path.join(os.path.expanduser("~"), ".config")
    path = os.path.join(base, "Aion2IncomeAssistant")
    os.makedirs(path, exist_ok=True)
    return path


@dataclass
class Settings:
    levels: dict = field(default_factory=lambda: {p: 0 for p in PROFESSIONS})
    tax_pct: float = 10.0
    fee_pct: float = 2.0
    overrides: dict = field(default_factory=dict)  # recipe id -> your in-game success %
    budget: float = 5_000_000
    horizon: float = 3.0
    risks: list = field(default_factory=lambda: ["Safe", "Moderate"])
    server_name: str = "Lumiel"

    @property
    def keep(self) -> float:
        return max(0.0, 1.0 - self.tax_pct / 100.0 - self.fee_pct / 100.0)

    @classmethod
    def load(cls, path: str) -> "Settings":
        s = cls()
        try:
            with open(path, encoding="utf-8") as f:
                raw = json.load(f)
        except (OSError, ValueError):
            return s
        for k in ("tax_pct", "fee_pct", "budget", "horizon"):
            if isinstance(raw.get(k), (int, float)) and raw[k] >= 0:
                setattr(s, k, float(raw[k]))
        if isinstance(raw.get("levels"), dict):
            for p in PROFESSIONS:
                v = raw["levels"].get(p)
                if isinstance(v, int) and 0 <= v <= 200:
                    s.levels[p] = v
        if isinstance(raw.get("overrides"), dict):
            s.overrides = {str(k): float(v) for k, v in raw["overrides"].items()
                           if isinstance(v, (int, float)) and 0 < v <= 100}
        if isinstance(raw.get("risks"), list):
            s.risks = [r for r in raw["risks"] if r in ("Safe", "Moderate", "Risky")]
        return s

    def save(self, path: str) -> None:
        tmp = path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump({"levels": self.levels, "tax_pct": self.tax_pct, "fee_pct": self.fee_pct,
                       "overrides": self.overrides, "budget": self.budget, "horizon": self.horizon,
                       "risks": self.risks, "server_name": self.server_name}, f, indent=1)
        os.replace(tmp, path)


@dataclass
class MarketData:
    """Everything downloaded in one refresh; analysis can be re-run on it offline."""
    server: dict
    rows: list
    fetched_at: float
    offline: bool = False
    details: dict = field(default_factory=dict)  # item key -> {"hourly", "buckets", "depth"}
    snapshots: dict = field(default_factory=dict)
    snapshot_count: int = 0
    warnings: list = field(default_factory=list)
    requests: int = 0


@dataclass
class Analysis:
    data: MarketData
    stats: dict
    ctx: CraftContext
    crafts: list
    ops: list


def faction_code(server: dict) -> str:
    return "light" if str(server.get("faction", "")).lower().startswith("ely") else "dark"


class Engine:
    def __init__(self, data_dir: str | None = None, client: ExchangeClient | None = None,
                 book: RecipeBook | None = None, workers: int = 4):
        self.data_dir = data_dir or default_data_dir()
        os.makedirs(self.data_dir, exist_ok=True)
        self.client = client or ExchangeClient()
        self.store = HistoryStore(os.path.join(self.data_dir, "history.db"))
        self.workers = workers
        self._book = book

    @property
    def book(self) -> RecipeBook:
        if self._book is None:
            self._book = load_book(self.data_dir, http_get=self._get_any)
        return self._book

    def _get_any(self, url):
        r = self.client.session.get(url, timeout=40)
        r.raise_for_status()
        return r.json()

    @property
    def settings_path(self) -> str:
        return os.path.join(self.data_dir, "settings.json")

    # ── download ─────────────────────────────────────────────────────────

    def download(self, settings: Settings, progress=lambda msg, frac: None) -> MarketData:
        progress("Finding Lumiel server…", 0.02)
        warnings = []
        start_requests = self.client.request_count
        try:
            server = self.client.find_server(settings.server_name)
        except ApiError as e:
            server = dict(LUMIEL_FALLBACK)
            warnings.append(f"Server list unavailable ({e}); using Lumiel id {server['server_id']}.")
        sid = int(server["server_id"])
        progress("Downloading market prices…", 0.06)
        offline = False
        try:
            rows = self.client.items(sid)
            fetched = time.time()
            self.store.save_market(sid, rows)
            self.store.add_snapshot(sid, rows)
        except ApiError as e:
            cached = self.store.load_market(sid)
            if not cached:
                raise ApiError(f"Could not reach aion2.exchange and no saved data yet: {e}") from e
            fetched, rows = cached
            offline = True
            warnings.append(f"aion2.exchange did not answer ({e}). Showing data saved "
                            f"{(time.time() - fetched) / 60:.0f} min ago.")
        book = self.book  # loads (and maybe updates) recipes once
        data = MarketData(server=server, rows=rows, fetched_at=fetched, offline=offline, warnings=warnings)
        data.snapshots = self.store.all_series(sid, time.time() - 7 * 86400)
        data.snapshot_count = self.store.snapshot_count(sid)

        # first pass without history to see which items matter
        progress("Finding candidates…", 0.12)
        first = self.analyse(data, settings, book)
        keys = self.candidate_keys(first)
        data.details = self.fetch_details(sid, keys, first, offline, progress)
        data.requests = self.client.request_count - start_requests
        return data

    def candidate_keys(self, a: Analysis) -> list[str]:
        """Items worth a closer look: what the best crafts sell and buy, plus the
        busiest tradable items (resell candidates), capped to stay polite."""
        ordered: list[str] = []

        def add(k):
            if k in a.stats and k not in ordered:
                ordered.append(k)
        for res in sorted(a.crafts, key=lambda c: c.profit, reverse=True)[:30]:
            add(res.out_key)
            if res.combo_key:
                add(res.combo_key)
            for line in res.lines:
                if line.source.how in ("buy", "bulk"):
                    add(line.key)
                if line.source.how == "morph" and line.source.via:
                    add(line.source.via)
        for od in {od for od, _ in a.ctx.book.morph.values()}:
            add(od)
        busy = sorted((s for s in a.stats.values() if s.ask and s.listings >= 3 and "(Bound)" not in s.name),
                      key=lambda s: (-(s.change_pct or 0) if (s.change_pct or 0) < -10 else 0, -s.listings))
        for s in busy:
            if len(ordered) >= MAX_DETAIL_ITEMS:
                break
            add(s.key)
        return ordered[:MAX_DETAIL_ITEMS]

    def fetch_details(self, sid, keys, first: Analysis, offline: bool, progress) -> dict:
        details: dict = {}

        def one(k):
            d = {}
            for kind, ttl, fn in (("hourly", HOURLY_TTL, self.client.price_history),
                                  ("buckets", BUCKETS_TTL, self.client.recent_buckets),
                                  ("depth", DEPTH_TTL, self.client.depth)):
                cached = self.store.cache_get(sid, k, kind, ttl if not offline else 10 * 86400)
                if cached is not None:
                    d[kind] = cached
                    continue
                if offline:
                    continue
                try:
                    body = fn(k, sid)
                    self.store.cache_put(sid, k, kind, body)
                    d[kind] = body
                except ApiError:
                    stale = self.store.cache_get(sid, k, kind, 10 * 86400)
                    if stale is not None:
                        d[kind] = stale
            return k, d

        done = 0
        with concurrent.futures.ThreadPoolExecutor(max_workers=self.workers) as ex:
            for k, d in ex.map(one, keys):
                details[k] = d
                done += 1
                progress(f"Price history {done}/{len(keys)}…", 0.15 + 0.8 * done / max(1, len(keys)))
        return details

    # ── analysis (no network) ────────────────────────────────────────────

    def analyse(self, data: MarketData, settings: Settings, book: RecipeBook | None = None) -> Analysis:
        book = book or self.book
        stats = {}
        for row in data.rows:
            key = str(row.get("item_key"))
            d = data.details.get(key, {})
            stats[key] = build_stats(row, hourly=d.get("hourly"), buckets=d.get("buckets"),
                                     snapshots=data.snapshots.get(key), depth=d.get("depth"))
            if key in book.items and book.items[key][0]:
                stats[key].name = stats[key].name or book.items[key][0]
        ctx = CraftContext(book, stats, settings.levels, faction_code(data.server), settings.keep,
                           settings.overrides)
        crafts = ctx.evaluate_all()
        ops = all_opportunities(ctx, crafts)
        return Analysis(data, stats, ctx, crafts, ops)

    def refresh(self, settings: Settings, progress=lambda msg, frac: None) -> Analysis:
        data = self.download(settings, progress)
        progress("Calculating…", 0.97)
        a = self.analyse(data, settings)
        progress("Done", 1.0)
        return a
