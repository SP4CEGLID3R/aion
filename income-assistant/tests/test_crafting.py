import pytest

from aion_income.crafting import CraftContext, batch_loss_probability, curve, success_chance
from aion_income.pricing import ItemStats
from aion_income.recipes import Recipe, RecipeBook


def stats(key, name, ask, listings=30, ref=None, sales=10.0):
    return ItemStats(key=key, name=name, ask=ask, listings=listings, ref=ref or ask, ref_basis="test",
                     history_points=48, sales_per_day=sales, sales_hours=24, volatility=0.05, pessimistic=(ref or ask) * 0.95)


def mini_book(recipes, morph=None, npc=None):
    items = {
        "ore": ("Pure Orichalcum Ore", "misc"), "od": ("Pure Odyle", "misc"), "ingot": ("Orichalcum Ingot", "misc"),
        "sword": ("Orichalcum Longsword", "weapon"), "ssword": ("Splendent Orichalcum Longsword", "weapon"),
        "abr": ("Abrasive (Bound)", "misc"), "elyos": ("Yggdrasil Thing", "misc"),
    }
    return RecipeBook(items=items, recipes=recipes, morph=morph or {"ore": ("od", 1)},
                      npc_prices=npc or {"Abrasive (Bound)": 400.0}, npc_estimated=set())


SWORD = Recipe(id="r1", profession="blacksmithing", faction="dark", level=3, tier="beginner", gold=1000,
               inputs=[("ore", 10, None), ("abr", 2, None)], out="sword", out_qty=1, combo_chance=0.25, combo="ssword")


def test_curve_is_monotonic_and_bounded():
    vals = [curve(g) for g in range(0, 80)]
    assert vals[0] == 0 and vals[-1] == 1
    assert all(b >= a for a, b in zip(vals, vals[1:]))


def test_success_matches_published_points():
    # Superior Healing Potion table: +0 72.88%, +10 79.60%, +60 90.89%
    book = RecipeBook(items={"p": ("Superior Healing Potion", "usable")}, recipes=[], morph={}, npc_prices={})
    r = Recipe(id="x", profession="alchemy", faction="dark", level=60, tier="intermediate", gold=0,
               inputs=[], out="p", out_qty=1, combo_chance=0, combo=None)
    assert success_chance(book, r, 60) == pytest.approx(0.7288, abs=1e-4)
    assert success_chance(book, r, 70) == pytest.approx(0.7960, abs=2e-3)
    assert success_chance(book, r, 120) == pytest.approx(0.9089, abs=1e-4)
    assert success_chance(book, r, 59) is None  # below the recipe level you can't craft it


def test_morph_used_when_odyle_is_cheaper():
    st = {"ore": stats("ore", "Pure Orichalcum Ore", 99_000), "od": stats("od", "Pure Odyle", 87_000),
          "sword": stats("sword", "Orichalcum Longsword", 2_000_000)}
    ctx = CraftContext(mini_book([SWORD]), st, {"blacksmithing": 10}, "dark", keep=0.88)
    src = ctx.cheapest_source("ore")
    assert src.how == "morph" and src.cost == 87_000


def test_material_bought_when_cheaper_than_odyle():
    st = {"ore": stats("ore", "Pure Orichalcum Ore", 3_099), "od": stats("od", "Pure Odyle", 5_499),
          "sword": stats("sword", "Orichalcum Longsword", 200_000)}
    ctx = CraftContext(mini_book([SWORD]), st, {"blacksmithing": 10}, "dark", keep=0.88)
    assert ctx.cheapest_source("ore").how == "buy"
    assert ctx.cheapest_source("ore").cost == 3_099


def test_expected_profit_includes_failures_tax_and_npc_items():
    st = {"ore": stats("ore", "Pure Orichalcum Ore", 100), "od": stats("od", "Pure Odyle", 500),
          "sword": stats("sword", "Orichalcum Longsword", 10_000)}
    ctx = CraftContext(mini_book([SWORD]), st, {"blacksmithing": 3}, "dark", keep=0.88)
    res = ctx.evaluate(SWORD)
    assert res.cost == 10 * 100 + 2 * 400 + 1000
    # no Splendent price -> combo counts as a normal item
    assert res.p_combo == 0
    assert res.ev == pytest.approx(res.success * 10_000 * 0.88)
    assert res.profit == pytest.approx(res.ev - res.cost)


def test_combo_raises_expected_value():
    st = {"ore": stats("ore", "Pure Orichalcum Ore", 100), "od": stats("od", "Pure Odyle", 500),
          "sword": stats("sword", "Orichalcum Longsword", 10_000), "ssword": stats("ssword", "Splendent", 50_000)}
    ctx = CraftContext(mini_book([SWORD]), st, {"blacksmithing": 3}, "dark", keep=0.88)
    res = ctx.evaluate(SWORD)
    assert res.p_combo == pytest.approx(res.success * 0.25 * res.combo_rate)
    assert res.p_fail + res.p_plain + res.p_combo == pytest.approx(1.0)
    assert res.profit > res.profit_plain_only


def test_level_too_low_or_unlearned_means_no_result():
    st = {"ore": stats("ore", "Pure Orichalcum Ore", 100), "sword": stats("sword", "Orichalcum Longsword", 10_000)}
    assert CraftContext(mini_book([SWORD]), st, {"blacksmithing": 2}, "dark", 0.88).evaluate(SWORD) is None
    assert CraftContext(mini_book([SWORD]), st, {}, "dark", 0.88).evaluate(SWORD) is None


def test_other_faction_recipes_are_ignored():
    light = Recipe(**{**SWORD.__dict__, "id": "r2", "faction": "light"})
    st = {"ore": stats("ore", "Pure Orichalcum Ore", 100), "sword": stats("sword", "Orichalcum Longsword", 10_000)}
    ctx = CraftContext(mini_book([light]), st, {"blacksmithing": 50}, "dark", 0.88)
    assert ctx.evaluate_all() == []


def test_override_success():
    st = {"ore": stats("ore", "Pure Orichalcum Ore", 100), "sword": stats("sword", "Orichalcum Longsword", 10_000)}
    ctx = CraftContext(mini_book([SWORD]), st, {"blacksmithing": 3}, "dark", 0.88, overrides={"r1": 50})
    assert ctx.evaluate(SWORD).success == 0.5


def test_thin_or_unpriced_output_is_shown_but_flagged():
    from aion_income.opportunities import RISKY, craft_opportunity
    st = {"ore": stats("ore", "Pure Orichalcum Ore", 100), "sword": stats("sword", "Orichalcum Longsword", 10_000, listings=1)}
    st["sword"].sales_per_day = None
    ctx = CraftContext(mini_book([SWORD]), st, {"blacksmithing": 3}, "dark", 0.88)
    res = ctx.evaluate(SWORD)
    assert res.priced and not res.price_ok         # one listing is no evidence of value...
    op = craft_opportunity(res, ctx)
    assert op.risk == RISKY and any("price is a guess" in r for r in op.reasons)   # ...so never above Risky
    del st["sword"]                                # nobody has ever listed it: cost only, no profit
    ctx = CraftContext(mini_book([SWORD]), st, {"blacksmithing": 3}, "dark", 0.88)
    res = ctx.evaluate(SWORD)
    assert not res.priced and res.profit == 0 and res.avg_cost_item == res.cost / res.success
    assert craft_opportunity(res, ctx) is None


def test_splendent_from_a_combo_is_used_for_the_next_tier_when_worth_more():
    up = Recipe(id="r2", profession="blacksmithing", faction="dark", level=3, tier="beginner", gold=0,
                inputs=[("ssword", 1, None), ("ore", 5, None)], out="xsword", out_qty=1, combo_chance=0, combo=None)
    st = {"ore": stats("ore", "Pure Orichalcum Ore", 100), "sword": stats("sword", "Orichalcum Longsword", 10_000),
          "xsword": stats("xsword", "Expert's Longsword", 200_000)}   # the Splendent itself never sells
    ctx = CraftContext(mini_book([SWORD, up]), st, {"blacksmithing": 3}, "dark", 0.88)
    res = next(c for c in ctx.evaluate_all() if c.out_key == "sword")
    assert res.combo_best == "craft" and res.combo_sell is None and res.combo_use_name == "Expert's Longsword"
    assert res.combo_use == pytest.approx((ctx.chance(up) * 200_000 * 0.88 - 500))   # minus 5 ore, failures included
    assert res.p_combo > 0 and res.profit > res.profit_plain_only
    st["ssword"] = stats("ssword", "Splendent", 500_000)            # now it sells for more than it's worth as a base
    ctx = CraftContext(mini_book([SWORD, up]), st, {"blacksmithing": 3}, "dark", 0.88)
    res = next(c for c in ctx.evaluate_all() if c.out_key == "sword")
    assert res.combo_best == "sell" and res.combo_sell > res.combo_use


def test_unbuyable_input_is_shown_but_never_recommended():
    from aion_income.opportunities import craft_opportunity
    r = Recipe(**{**SWORD.__dict__, "inputs": [("elyos", 1, None)]})
    st = {"sword": stats("sword", "Orichalcum Longsword", 10_000)}
    ctx = CraftContext(mini_book([r]), st, {"blacksmithing": 3}, "dark", 0.88)
    res = ctx.evaluate(r)
    assert res.missing == ["Yggdrasil Thing"] and res.lines[0].source.how == "missing"
    assert craft_opportunity(res, ctx) is None          # its cost is incomplete: never a recommendation
    assert ctx.cheapest_source("sword") is None or ctx.cheapest_source("sword").how != "craft"  # never feeds others


def test_crafted_intermediate_used_when_cheaper():
    ingot = Recipe(id="ri", profession="blacksmithing", faction="dark", level=1, tier="beginner", gold=0,
                   inputs=[("ore", 2, None)], out="ingot", out_qty=1, combo_chance=0, combo=None)
    sword = Recipe(**{**SWORD.__dict__, "inputs": [("ingot", 5, None)], "combo": None, "combo_chance": 0})
    st = {"ore": stats("ore", "Pure Orichalcum Ore", 100), "ingot": stats("ingot", "Orichalcum Ingot", 5_000),
          "sword": stats("sword", "Orichalcum Longsword", 100_000)}
    ctx = CraftContext(mini_book([ingot, sword]), st, {"blacksmithing": 10}, "dark", 0.88)
    src = ctx.cheapest_source("ingot")
    assert src.how == "craft"
    assert src.cost == pytest.approx(200 / ctx.chance(ingot))


def test_batch_loss_probability():
    st = {"ore": stats("ore", "Pure Orichalcum Ore", 100), "sword": stats("sword", "Orichalcum Longsword", 10_000)}
    ctx = CraftContext(mini_book([SWORD]), st, {"blacksmithing": 3}, "dark", 0.88)
    res = ctx.evaluate(SWORD)
    p = batch_loss_probability(res, 10)
    assert 0 <= p < 0.01  # needs only ~3 of 10 successes to break even


def test_batch_loss_counts_combo_results():
    import dataclasses
    st = {"ore": stats("ore", "Pure Orichalcum Ore", 100), "sword": stats("sword", "Orichalcum Longsword", 10_000)}
    ctx = CraftContext(mini_book([SWORD]), st, {"blacksmithing": 3}, "dark", 0.88)
    res = ctx.evaluate(SWORD)
    # plain result worth nothing, combo result worth a lot: profitable only through combos
    lottery = dataclasses.replace(res, plain_value=0.0, combo_value=res.cost * 20, p_fail=0.2, p_plain=0.6, p_combo=0.2)
    p = batch_loss_probability(lottery, 10)
    assert abs(p - 0.8 ** 10) < 1e-9          # loses only if no combo at all in 10 tries
    never = dataclasses.replace(res, plain_value=0.0, combo_value=0.0)
    assert batch_loss_probability(never, 10) == 1.0


def test_splendent_without_sales_is_not_counted():
    st = {"ore": stats("ore", "Pure Orichalcum Ore", 100), "od": stats("od", "Pure Odyle", 500),
          "sword": stats("sword", "Orichalcum Longsword", 10_000),
          "ssword": stats("ssword", "Splendent", 50_000, sales=None)}
    ctx = CraftContext(mini_book([SWORD]), st, {"blacksmithing": 3}, "dark", keep=0.88)
    res = ctx.evaluate(SWORD)
    assert res.p_combo == 0 and res.profit == pytest.approx(res.profit_plain_only)
