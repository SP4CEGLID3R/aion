import os
import time

import pytest

pytest.importorskip("PySide6")

from PySide6.QtCore import Qt  # noqa: E402
from PySide6.QtWidgets import QApplication  # noqa: E402

from aion_income.gui.main_window import MainWindow  # noqa: E402
from aion_income.gui.widgets import parse_kinah  # noqa: E402


@pytest.fixture(scope="module")
def app():
    from aion_income.gui.theme import apply_theme
    a = QApplication.instance() or QApplication([])
    apply_theme(a)
    return a


def wait_refresh(app, win, timeout=60):
    end = time.time() + timeout
    while time.time() < end:
        app.processEvents()
        if not win.is_refreshing() and win.analysis is not None:
            return
        time.sleep(0.02)
    raise AssertionError("refresh did not finish")


def test_parse_kinah():
    assert parse_kinah("5m") == 5e6
    assert parse_kinah("5.5kk") == 5.5e6
    assert parse_kinah("500k") == 5e5
    assert parse_kinah("12,000,000") == 12e6
    assert parse_kinah("2b") == 2e9
    assert parse_kinah("abc") is None and parse_kinah("") is None


def test_window_end_to_end(app, engine, settings, fake, tmp_path):
    fake.dips = {"610610015": 0.75}
    win = MainWindow(engine, settings, auto_refresh=False)
    win.show()
    assert win.tabs.count() == 5
    win.refresh_btn.click()
    wait_refresh(app, win)
    assert "Updated" in win.status.text()
    assert win.best_model.rowCount() > 0
    assert win.craft_model.rowCount() > 0
    assert win.market_model.rowCount() == len(fake.rows)

    # risk filters
    for cb in win.risk_boxes.values():
        cb.setChecked(False)
    assert win.best_proxy.rowCount() == 0
    win.risk_boxes["Risky"].setChecked(True)
    shown = [win.best_proxy.index(i, 0).data(Qt.UserRole + 1) for i in range(win.best_proxy.rowCount())]
    assert shown and all(o.risk == "Risky" for o in shown)
    for cb in win.risk_boxes.values():
        cb.setChecked(True)
    total = win.best_proxy.rowCount()
    win.fast_box.setChecked(True)
    assert all(win.best_proxy.index(i, 0).data(Qt.UserRole + 1).fast for i in range(win.best_proxy.rowCount()))
    win.fast_box.setChecked(False)
    assert win.best_proxy.rowCount() == total

    # sorting by profit per day, descending by default
    vals = [win.best_proxy.index(i, 3).data(Qt.UserRole) for i in range(win.best_proxy.rowCount())]
    assert vals == sorted(vals, reverse=True)

    # selecting a row fills the explanation
    win.best_view.selectRow(0)
    app.processEvents()
    assert "What to do" in win.best_detail.toHtml()

    # plan follows the budget
    win.tabs.setCurrentIndex(1)
    win.budget_edit.setText("2m")
    win.budget_edit.editingFinished.emit()
    assert win.settings.budget == 2e6
    assert win.current_plan.capital <= 2e6
    assert "Spend" in win.plan_summary.text() or "Nothing fits" in win.plan_summary.text()
    win.budget_edit.setText("lots")
    win.budget_edit.editingFinished.emit()
    assert win.settings.budget == 2e6  # bad input ignored

    # crafting levels to 0 -> no crafts, without a new download
    n = len(fake.requests)
    for sp in win.level_spins.values():
        sp.setValue(0)
    win.recalculate()
    assert win.craft_model.rowCount() == 0
    assert all(o.method == "Resell" for o in win.best_model.rows)
    assert len(fake.requests) == n
    assert not win.banner.isHidden()

    # market tab detail + chart
    win.tabs.setCurrentIndex(3)
    win.market_view.selectRow(0)
    app.processEvents()
    assert "Cheapest now" in win.market_detail.toHtml()

    # settings were saved
    assert os.path.exists(engine.settings_path)

    out = os.environ.get("AION_SCREENSHOT_DIR")
    if out:
        for lv in win.level_spins.values():
            lv.setValue(60)
        win.recalculate()
        win.resize(1440, 900)
        for i, name in enumerate(["best", "plan", "craft", "market", "settings"]):
            win.tabs.setCurrentIndex(i)
            for view in (win.best_view, win.plan_view, win.craft_view, win.market_view):
                if view.model().rowCount():
                    view.selectRow(0)
            app.processEvents()
            win.grab().save(os.path.join(out, f"{name}.png"))
    win.close()


def test_refresh_failure_shows_message(app, engine, settings, fake, monkeypatch):
    fake.fail.update({"items"})
    shown = {}
    from PySide6.QtWidgets import QMessageBox
    monkeypatch.setattr(QMessageBox, "warning", lambda *a: shown.setdefault("msg", a[2]))
    win = MainWindow(engine, settings, auto_refresh=False)
    win.refresh()
    end = time.time() + 30
    while win.is_refreshing() and time.time() < end:
        app.processEvents()
        time.sleep(0.02)
    app.processEvents()
    assert "aion2.exchange" in shown.get("msg", "")
    assert win.refresh_btn.isEnabled()
    win.close()
