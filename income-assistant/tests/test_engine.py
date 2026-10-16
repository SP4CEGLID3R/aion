import json
import time

import pytest

from aion_income.api import ApiError, ExchangeClient
from aion_income.engine import Settings
from aion_income.opportunities import RISK_ORDER
from aion_income.planner import make_plan
from aion_income.recipes import compact_graph, load_book


def test_find_lumiel(fake):
    c = ExchangeClient(fake.base, min_interval=0, retry_waits=(0.01,))
    s = c.find_server("Lumiel")
    assert s["server_id"] == 12104 and s["faction"] == "Asmodian"
    with pytest.raises(ApiError):
        c.find_server("Nope")


def test_items_paginates_within_api_limit(fake):
    c = ExchangeClient(fake.base, min_interval=0, retry_waits=(0.01,))
    rows = c.items(12104)
    assert len(rows) == len(fake.rows)
    assert len({r["item_key"] for r in rows}) == len(rows)
    assert all("limit=200" in r or "/items?" not in r for r in fake.requests)


def test_server_errors_retry_then_fail(fake):
    fake.fail.add("servers")
    c = ExchangeClient(fake.base, min_interval=0, retry_waits=(0.01, 0.01))
    with pytest.raises(ApiError):
        c.servers()
    assert sum(1 for r in fake.requests if r.startswith("/api/servers")) == 3


def test_client_error_is_not_retried(fake):
    c = ExchangeClient(fake.base, min_interval=0, retry_waits=(0.01, 0.01))
    with pytest.raises(ApiError):
        c.get_json("/nothing-here")
    assert len(fake.requests) == 1


def test_full_refresh(engine, settings, fake):
    fake.dips = {"610610015": 0.75}  # Pure Odyle listed cheap
    a = engine.refresh(settings)
    assert len(a.stats) == len(fake.rows)
    assert a.crafts and a.ops
    assert a.data.snapshot_count == 1
    # every profit is real money after tax, and sorted by profit per day
    assert all(o.profit_unit > 0 for o in a.ops)
    days = [o.profit_day for o in a.ops]
    assert days == sorted(days, reverse=True)
    assert all(o.risk in RISK_ORDER for o in a.ops)
    # never built on low-stock items
    for o in a.ops:
        assert o.stats.listings >= 3 or o.stats.units_sold_28d > 0
    # Odyle cheaper than the ore -> ore comes from Substance Morph
    src = a.ctx.cheapest_source("610620015")
    assert src.how == "morph"
    # ...and the plan respects the budget
    p = make_plan([o for o in a.ops if o.risk in settings.risks], 5e6, 3)
    assert p.capital <= 5e6


def test_second_refresh_uses_cache(engine, settings, fake):
    engine.refresh(settings)
    n = len(fake.requests)
    engine.refresh(settings)
    second = len(fake.requests) - n
    assert second < 15, second  # only the market list, history comes from cache


def test_offline_fallback(engine, settings, fake):
    engine.refresh(settings)
    fake.fail.update({"items", "details", "servers"})
    a = engine.refresh(settings)
    assert a.data.offline and a.data.warnings
    assert len(a.stats) == len(fake.rows)


def test_first_run_offline_is_a_clear_error(engine, settings, fake):
    fake.fail.update({"items"})
    with pytest.raises(ApiError):
        engine.refresh(settings)


def test_levels_change_results_without_network(engine, settings, fake):
    a = engine.refresh(settings)
    n = len(fake.requests)
    s0 = Settings()
    b = engine.analyse(a.data, s0)
    assert b.crafts == [] and all(o.method == "Resell" for o in b.ops)
    assert len(fake.requests) == n


def test_higher_tax_means_less_profit(engine, settings):
    a = engine.refresh(settings)
    s2 = Settings(**{**vars(settings), "tax_pct": 20.0})
    b = engine.analyse(a.data, s2)
    pa = {c.recipe.id: c.profit for c in a.crafts}
    for c in b.crafts:
        if c.recipe.id in pa:
            assert c.profit <= pa[c.recipe.id] + 1e-6


def test_settings_roundtrip_and_bad_file(tmp_path):
    p = tmp_path / "s.json"
    s = Settings()
    s.levels["alchemy"] = 77
    s.overrides = {"123": 88.5}
    s.budget = 12e6
    s.save(str(p))
    t = Settings.load(str(p))
    assert t.levels["alchemy"] == 77 and t.overrides == {"123": 88.5} and t.budget == 12e6
    p.write_text("{broken")
    assert Settings.load(str(p)).levels["alchemy"] == 0
    p.write_text(json.dumps({"levels": {"alchemy": "x"}, "tax_pct": -5, "risks": ["Bogus", "Safe"]}))
    t = Settings.load(str(p))
    assert t.levels["alchemy"] == 0 and t.tax_pct == 10.0 and t.risks == ["Safe"]


def test_bundled_recipes_look_right(book):
    dark = book.for_faction("dark")
    assert len(dark) > 500
    assert {r.profession for r in dark} == {"alchemy", "blacksmithing", "tailoring", "jewelcrafting", "cooking"}
    # every Odyle grade morphs into its own grade of ore
    names = {book.name(k): book.name(od) for k, (od, _) in book.morph.items()}
    assert names["Pure Orichalcum Ore"] == "Pure Odyle"
    assert names["Radiant Orichalcum Ore"] == "Radiant Odyle"
    assert names["Orichalcum Ore"] == "Odyle"


def test_recipe_update_falls_back_on_bad_download(tmp_path):
    def bad(url):
        raise OSError("offline")
    assert load_book(str(tmp_path), http_get=bad).source == "bundled"

    def stub(url):
        return {"items": {}, "recipes": {}}
    assert load_book(str(tmp_path), http_get=stub).source == "bundled"  # never replaced by a stub


def test_compact_graph_shape():
    g = {"items": {"1": {"n": "A", "mc": "misc"}, "2": {"n": "B", "mc": "weapon"}, "3": {"n": "unused"}},
         "recipes": {"9": {"p": "blacksmithing", "qr": "dark", "ml": 5, "mg": "beginner", "gold": 10,
                           "in": [["1", 2, None]], "out": "2", "outq": 1, "cp": 2500, "combo": None},
                     "10": {"in": []}}}
    d = compact_graph(g)
    assert set(d["items"]) == {"1", "2"} and len(d["recipes"]) == 1
    assert d["recipes"][0]["in"] == [["1", 2, None]]


def test_snapshots_accumulate(engine, settings, fake):
    engine.refresh(settings)
    for r in fake.rows:
        r["captured_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(time.time() + 600))
    a = engine.refresh(settings)
    assert a.data.snapshot_count == 2
