import pytest

from aion_income.crafting import CraftContext, combo_success
from aion_income.opportunities import craft_opportunity
from aion_income.recipes import Recipe
from aion_income.shopping import shopping_list

from test_crafting import SWORD, mini_book, stats

UP = Recipe(id="r2", profession="blacksmithing", faction="dark", level=3, tier="beginner", gold=500,
            inputs=[("ssword", 1, None), ("ore", 5, None)], out="xsword", out_qty=1, combo_chance=0, combo=None)


def ctx_for(st, recipes):
    return CraftContext(mini_book(recipes), st, {"blacksmithing": 3}, "dark", 0.88)


def test_splendent_base_can_be_made_through_combo_procs():
    st = {"ore": stats("ore", "Pure Orichalcum Ore", 100), "sword": stats("sword", "Orichalcum Longsword", 10_000),
          "xsword": stats("xsword", "Expert's Longsword", 400_000)}           # no Splendent on the market
    ctx = ctx_for(st, [SWORD, UP])
    src = ctx.cheapest_source("ssword")
    assert src and src.how == "combo" and src.via == "r1"
    s = ctx.chance(SWORD)
    p = s * 0.25 * combo_success(ctx.book, SWORD, 3, s)
    attempt = ctx.run_inputs(SWORD)[0]
    plain = st["sword"].sell_pessimistic * 0.88
    assert src.cost == pytest.approx(max(0.0, (attempt - (s - p) * plain) / p))
    res = ctx.evaluate(UP)                                                    # the next tier is no longer skipped
    assert res is not None and res.lines[0].source.how == "combo"


def test_shopping_list_breaks_everything_down_to_what_you_buy():
    st = {"ore": stats("ore", "Pure Orichalcum Ore", 100), "od": stats("od", "Pure Odyle", 50),
          "sword": stats("sword", "Orichalcum Longsword", 10_000), "xsword": stats("xsword", "Expert's Longsword", 400_000)}
    ctx = ctx_for(st, [SWORD, UP])
    res = ctx.evaluate(UP)
    op = craft_opportunity(res, ctx)
    sl = shopping_list(ctx, [(op, 4)])
    s_up, s = ctx.chance(UP), ctx.chance(SWORD)
    p = s * 0.25 * combo_success(ctx.book, SWORD, 3, s)
    sword_tries = 4 / p                                    # Splendents come from crafting the lower tier
    od = sl.materials["od"]                                # ore is cheaper as morphed Odyle
    assert "ore" not in sl.materials and od.qty == pytest.approx(4 * 5 + sword_tries * 10)
    assert sl.materials["abr"].qty == pytest.approx(sword_tries * 2)          # NPC abrasive for the lower tier
    assert sl.fees == pytest.approx(4 * 500 + sword_tries * 1000)
    assert sl.byproducts["sword"][1] == pytest.approx((s - p) * sword_tries)  # normal swords to sell
    assert sl.total == pytest.approx(sum(m.buy_qty * m.unit for m in sl.materials.values()) + sl.fees)
    assert "TOTAL" in sl.as_text()
    assert s_up > 0


def test_shopping_list_for_a_resell():
    from aion_income.opportunities import Opportunity
    st = stats("x", "Thing", 1000)
    op = Opportunity(method="Resell", name="Thing", key="x", cost_unit=1000, revenue_unit=1300, profit_unit=300,
                     profit_pess_unit=100, market_per_day=10, your_per_day=3, sell_price=1300, buy_price=1000, stats=st)
    ctx = ctx_for({"x": st}, [])
    sl = shopping_list(ctx, [(op, 7)])
    assert sl.materials["x"].buy_qty == 7 and sl.total == 7000


def test_splendent_shows_its_craft_from_scratch_cost_even_when_buying_is_cheaper():
    st = {"ore": stats("ore", "Pure Orichalcum Ore", 100), "sword": stats("sword", "Orichalcum Longsword", 2_000),
          "ssword": stats("ssword", "Splendent Orichalcum Longsword", 1_000),          # cheap on the market
          "xsword": stats("xsword", "Expert's Longsword", 400_000)}
    ctx = ctx_for(st, [SWORD, UP])
    assert ctx.cheapest_source("ssword").how == "buy"
    made = ctx.made["ssword"]
    assert made.how == "combo" and made.cost > 1_000
    res = next(c for c in ctx.evaluate_all() if c.out_key == "sword")
    assert res.combo_cost == pytest.approx(made.cost) and res.combo_price == 1_000


def test_every_splendent_gets_its_own_row():
    st = {"ore": stats("ore", "Pure Orichalcum Ore", 100), "sword": stats("sword", "Orichalcum Longsword", 2_000),
          "ssword": stats("ssword", "Splendent Orichalcum Longsword", 90_000)}
    ctx = ctx_for(st, [SWORD])
    crafts = ctx.evaluate_all()
    rows = ctx.splendent_rows(crafts)
    assert [r.out_key for r in rows] == ["ssword"]
    sp = rows[0]
    s = ctx.chance(SWORD)
    assert sp.success == pytest.approx(s * 0.25 * combo_success(ctx.book, SWORD, 3, s))
    assert sp.cost == pytest.approx(ctx.made["ssword"].cost) and sp.profit == pytest.approx(sp.net - sp.cost)
    assert sp.lines[0].qty == pytest.approx(SWORD.inputs[0][1] / sp.success)   # ingredients per piece
