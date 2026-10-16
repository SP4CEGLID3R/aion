"""Convert a gamers4.life crafting graph into the compact recipe file the app ships.

Usage: python tools/build_recipe_data.py path/to/graph.json

The app can also refresh the graph itself at runtime (see aion_income/recipes.py);
both paths go through recipes.compact_graph so they produce the same shape.
"""
import json
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))

from aion_income.recipes import compact_graph  # noqa: E402

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "aion_income", "data", "recipes.json")


def main():
    if len(sys.argv) != 2:
        print(__doc__)
        sys.exit(2)
    with open(sys.argv[1], encoding="utf-8") as f:
        graph = json.load(f)
    data = compact_graph(graph)
    with open(OUT, "w", encoding="utf-8") as f:
        json.dump(data, f, separators=(",", ":"), ensure_ascii=False)
    print(f"wrote {OUT}: {len(data['items'])} items, {len(data['recipes'])} recipes")


if __name__ == "__main__":
    main()
