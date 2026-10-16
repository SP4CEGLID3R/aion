import os
import sys

import pytest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from fake_exchange import FakeExchange  # noqa: E402

from aion_income.api import ExchangeClient  # noqa: E402
from aion_income.engine import Engine, Settings  # noqa: E402
from aion_income.recipes import _load_json, book_from_compact  # noqa: E402


@pytest.fixture
def fake():
    fx = FakeExchange()
    fx.base = fx.start()
    yield fx
    fx.stop()


@pytest.fixture
def book():
    return book_from_compact(_load_json("recipes.json"), "bundled")


@pytest.fixture
def engine(fake, book, tmp_path):
    client = ExchangeClient(fake.base, min_interval=0, retry_waits=(0.01,))
    eng = Engine(str(tmp_path), client=client, book=book)
    yield eng
    eng.store.close()


@pytest.fixture
def settings():
    s = Settings()
    s.levels = {p: 60 for p in s.levels}
    return s
