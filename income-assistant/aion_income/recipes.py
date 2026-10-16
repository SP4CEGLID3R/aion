"""Recipe database, Substance Morph table and NPC shop prices.

Recipes come from the gamers4.life crafting graph (the same data the community
crafting calculators use). A compact copy ships with the app; when possible the
app downloads a fresh graph once a week so new patches are picked up.
"""
from __future__ import annotations

import json
import os
import time
from dataclasses import dataclass, field

DATA_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "data")
GRAPH_URL = "https://gamers4.life/aion-2/database/crafting-graph.json"
GRAPH_MAX_AGE = 7 * 24 * 3600

# The graph's profession ids and what the game calls them.
PROFESSIONS = {
    "blacksmithing": "Weaponsmithing (swords, maces, guards)",
    "tailoring": "Armorsmithing (armor, cloaks)",
    "jewelcrafting": "Handicrafting (jewelry, bows, staffs)",
    "alchemy": "Alchemy (potions, scrolls, spellbooks)",
    "cooking": "Cooking",
}

KINA_KEY = "930100030"  # "Kina (All)" used as a recipe input: costs its quantity in kinah


@dataclass
class Recipe:
    id: str
    profession: str
    faction: str  # "light" (Elyos) or "dark" (Asmodian)
    level: int
    tier: str  # "beginner" | "intermediate"
    gold: int
    inputs: list  # [(item_key, qty, alt_key_or_None)]
    out: str
    out_qty: int
    combo_chance: float  # chance of a combo (Splendent) attempt after a success
    combo: str | None


@dataclass
class RecipeBook:
    items: dict  # key -> (name, main_category)
    recipes: list
    morph: dict  # material key -> (odyle key, qty)
    npc_prices: dict  # item name -> kinah
    npc_estimated: set = field(default_factory=set)
    source: str = "bundled"

    def name(self, key: str) -> str:
        it = self.items.get(str(key))
        return it[0] if it else str(key)

    def category(self, key: str) -> str | None:
        it = self.items.get(str(key))
        return it[1] if it else None

    def for_faction(self, faction: str) -> list:
        return [r for r in self.recipes if r.faction == faction]


def compact_graph(graph: dict) -> dict:
    """Keep only what the calculator needs from a gamers4.life graph."""
    used = set()
    recipes = []
    for rid, r in graph["recipes"].items():
        if not r.get("in"):
            continue
        ins = [[str(i[0]), int(i[1]), (str(i[2]) if len(i) > 2 and i[2] else None)] for i in r["in"]]
        rec = {
            "id": str(rid),
            "p": r.get("p"),
            "f": r.get("qr"),
            "ml": int(r.get("ml") or 0),
            "mg": r.get("mg"),
            "gold": int(r.get("gold") or 0),
            "in": ins,
            "out": str(r["out"]),
            "outq": int(r.get("outq") or 1),
            "cp": int(r.get("cp") or 0),
            "combo": str(r["combo"]) if r.get("combo") else None,
        }
        recipes.append(rec)
        used.add(rec["out"])
        if rec["combo"]:
            used.add(rec["combo"])
        for k, _, alt in ins:
            used.add(k)
            if alt:
                used.add(alt)
    items = {}
    for k, it in graph["items"].items():
        if str(k) in used:
            items[str(k)] = [it.get("n") or str(k), it.get("mc")]
    return {"items": items, "recipes": recipes}


def _load_json(name: str):
    with open(os.path.join(DATA_DIR, name), encoding="utf-8") as f:
        return json.load(f)


def book_from_compact(data: dict, source: str) -> RecipeBook:
    recipes = [
        Recipe(
            id=r["id"], profession=r["p"], faction=r["f"], level=r["ml"], tier=r["mg"], gold=r["gold"],
            inputs=[(k, q, a) for k, q, a in r["in"]], out=r["out"], out_qty=r["outq"],
            combo_chance=r["cp"] / 10000.0, combo=r["combo"],
        )
        for r in data["recipes"]
    ]
    items = {k: (v[0], v[1]) for k, v in data["items"].items()}
    morph = {k: (v["odyle"], int(v.get("qty", 1))) for k, v in _load_json("morph.json").items()}
    npc = _load_json("npc_prices.json")
    return RecipeBook(
        items=items, recipes=recipes, morph=morph,
        npc_prices={k: float(v) for k, v in npc["prices"].items()},
        npc_estimated=set(npc.get("estimated", [])), source=source,
    )


def load_book(cache_dir: str | None = None, http_get=None) -> RecipeBook:
    """Bundled recipes, replaced by a fresh download when one is available.

    http_get(url) -> parsed JSON; any failure keeps the bundled/cached copy.
    """
    if cache_dir:
        cached = os.path.join(cache_dir, "recipes_cache.json")
        fresh = os.path.exists(cached) and time.time() - os.path.getmtime(cached) < GRAPH_MAX_AGE
        if not fresh and http_get is not None:
            try:
                graph = http_get(GRAPH_URL)
                data = compact_graph(graph)
                if len(data["recipes"]) > 500:  # sanity: never replace good data with a stub
                    tmp = cached + ".tmp"
                    with open(tmp, "w", encoding="utf-8") as f:
                        json.dump(data, f, separators=(",", ":"))
                    os.replace(tmp, cached)
            except Exception:
                pass
        if os.path.exists(cached):
            try:
                with open(cached, encoding="utf-8") as f:
                    return book_from_compact(json.load(f), "downloaded")
            except Exception:
                pass
    return book_from_compact(_load_json("recipes.json"), "bundled")
