"""Main window: refresh bar on top, five tabs below."""
from __future__ import annotations

import copy
import html
import time

from PySide6.QtCore import QObject, Qt, QThread, QTimer, Signal, Slot
from PySide6.QtWidgets import (
    QCheckBox, QComboBox, QDoubleSpinBox, QFormLayout, QFrame, QGridLayout, QGroupBox, QHBoxLayout, QLabel,
    QLineEdit, QMainWindow, QMessageBox, QProgressBar, QPushButton, QSpinBox, QSplitter, QTabWidget,
    QTextBrowser, QVBoxLayout, QWidget,
)

from ..crafting import batch_loss_probability
from ..engine import Analysis, Engine, Settings
from ..opportunities import MARKET_SHARE, RISK_ORDER, speed_text
from ..planner import holding_tiers, make_plan
from ..pricing import history_stats
from ..recipes import PROFESSIONS
from ..store import parse_ts
from .widgets import (
    RISK_COLORS, Column, PriceChart, kinah, make_table, parse_kinah, pct, selected_row, short,
)

HORIZONS = [("1 day", 1.0), ("3 days", 3.0), ("7 days", 7.0)]


class RefreshWorker(QObject):
    progress = Signal(str, float)
    finished = Signal(object)
    failed = Signal(str)

    def __init__(self, engine: Engine, settings: Settings):
        super().__init__()
        self.engine = engine
        self.settings = settings

    @Slot()
    def run(self):
        try:
            a = self.engine.refresh(self.settings, lambda m, f: self.progress.emit(m, f))
            self.finished.emit(a)
        except Exception as e:  # shown to the user, never crashes the app
            self.failed.emit(str(e))


def esc(s) -> str:
    return html.escape(str(s))


def risk_badge(risk: str) -> str:
    return f'<span style="color:{RISK_COLORS.get(risk, "#000")};font-weight:bold">{esc(risk)}</span>'


class MainWindow(QMainWindow):
    def __init__(self, engine: Engine, settings: Settings | None = None, auto_refresh: bool = True):
        super().__init__()
        self.engine = engine
        self.settings = settings or Settings.load(engine.settings_path)
        self.analysis: Analysis | None = None
        self._thread = None
        self._worker = None
        self.setWindowTitle("Aion 2 Income Assistant · Lumiel")
        self.resize(1280, 800)

        central = QWidget()
        root = QVBoxLayout(central)
        root.addLayout(self._build_top_bar())
        self.banner = QLabel()
        self.banner.setWordWrap(True)
        self.banner.setStyleSheet("background:#fff4d6;color:#5a4500;padding:6px;border-radius:4px")
        self.banner.hide()
        root.addWidget(self.banner)

        self.tabs = QTabWidget()
        self.tabs.addTab(self._build_best_tab(), "Best ways to earn")
        self.tabs.addTab(self._build_plan_tab(), "My plan")
        self.tabs.addTab(self._build_craft_tab(), "Crafting")
        self.tabs.addTab(self._build_market_tab(), "Market")
        self.tabs.addTab(self._build_settings_tab(), "Settings")
        root.addWidget(self.tabs, 1)
        self.setCentralWidget(central)

        self._recalc_timer = QTimer(self)
        self._recalc_timer.setSingleShot(True)
        self._recalc_timer.setInterval(400)
        self._recalc_timer.timeout.connect(self.recalculate)

        self._update_banner()
        if auto_refresh:
            QTimer.singleShot(200, self.refresh)

    # ── top bar ──────────────────────────────────────────────────────────

    def _build_top_bar(self):
        bar = QHBoxLayout()
        title = QLabel("<b>Lumiel</b> market (Asmodian) · data: aion2.exchange")
        self.refresh_btn = QPushButton("⟳  Refresh")
        self.refresh_btn.setMinimumHeight(32)
        self.refresh_btn.setStyleSheet("font-weight:bold;padding:4px 18px")
        self.refresh_btn.clicked.connect(self.refresh)
        self.progress = QProgressBar()
        self.progress.setRange(0, 100)
        self.progress.setMaximumWidth(220)
        self.progress.hide()
        self.status = QLabel("Not loaded yet")
        bar.addWidget(title)
        bar.addStretch(1)
        bar.addWidget(self.status)
        bar.addWidget(self.progress)
        bar.addWidget(self.refresh_btn)
        return bar

    # ── tab 1: best ways to earn ─────────────────────────────────────────

    def _risk_boxes(self):
        boxes = {}
        row = QHBoxLayout()
        row.addWidget(QLabel("Show:"))
        tips = {
            "Safe": "Plenty of sales and stable prices; still profitable if prices drop to their recent low.",
            "Moderate": "Profitable on average with decent evidence, but could lose a little if the market dips.",
            "Risky": "Thin data, big price swings, slow selling or depends on combo luck.",
        }
        for r in ("Safe", "Moderate", "Risky"):
            cb = QCheckBox(r)
            cb.setChecked(r in self.settings.risks)
            cb.setStyleSheet(f"color:{RISK_COLORS[r]};font-weight:bold")
            cb.setToolTip(tips[r])
            cb.toggled.connect(self._risks_changed)
            boxes[r] = cb
            row.addWidget(cb)
        return row, boxes

    def _build_best_tab(self):
        w = QWidget()
        lay = QVBoxLayout(w)
        filt, self.risk_boxes = self._risk_boxes()
        filt.addSpacing(20)
        filt.addWidget(QLabel("Method:"))
        self.method_box = QComboBox()
        self.method_box.addItems(["All", "Craft", "Resell"])
        self.method_box.currentIndexChanged.connect(self._apply_best_filter)
        filt.addWidget(self.method_box)
        self.fast_box = QCheckBox("Fast sellers only (sells within a day)")
        self.fast_box.toggled.connect(self._apply_best_filter)
        filt.addWidget(self.fast_box)
        self.best_search = QLineEdit()
        self.best_search.setPlaceholderText("Search item…")
        self.best_search.textChanged.connect(self._apply_best_filter)
        filt.addWidget(self.best_search, 1)
        lay.addLayout(filt)

        cols = [
            Column("Risk", lambda o: RISK_ORDER[o.risk], lambda o: o.risk, color=lambda o: RISK_COLORS[o.risk],
                   tip=lambda o: "\n".join(o.reasons) or "No warnings"),
            Column("Method", lambda o: o.method),
            Column("Item", lambda o: o.name.lower(), lambda o: o.name, width=260),
            Column("Profit / day", lambda o: o.profit_day, lambda o: kinah(o.profit_day), align="right",
                   tip=lambda o: "What you can expect to earn per day, limited by how fast it sells"),
            Column("Profit / item", lambda o: o.profit_unit, lambda o: kinah(o.profit_unit), align="right",
                   tip=lambda o: "Per item bought (per craft attempt for crafts), after market tax and fee"),
            Column("ROI", lambda o: o.roi, lambda o: pct(o.roi), align="right"),
            Column("Sells in", lambda o: o.sell_days if o.sell_days is not None else 1e9,
                   lambda o: speed_text(o.sell_days), align="right",
                   tip=lambda o: "Time for a one-day batch to sell at the suggested price"),
            Column("Buy now", lambda o: o.batch, lambda o: f"{o.batch}", align="right",
                   tip=lambda o: "Suggested amount: about one day of what you can sell"),
            Column("Cost / item", lambda o: o.cost_unit, lambda o: kinah(o.cost_unit), align="right"),
            Column("Sell at", lambda o: o.sell_price, lambda o: kinah(o.sell_price), align="right"),
            Column("Why / warnings", lambda o: len(o.reasons),
                   lambda o: "; ".join(o.reasons[:2]) if o.reasons else "; ".join(o.good[:1])),
        ]
        self.best_model, self.best_proxy, self.best_view = make_table(cols)
        self.best_view.sortByColumn(3, Qt.DescendingOrder)
        self.best_view.selectionModel().selectionChanged.connect(self._best_selected)
        self.best_detail = QTextBrowser()
        split = QSplitter(Qt.Vertical)
        split.addWidget(self.best_view)
        split.addWidget(self.best_detail)
        split.setSizes([480, 220])
        lay.addWidget(split, 1)
        self.best_count = QLabel()
        lay.addWidget(self.best_count)
        return w

    def _apply_best_filter(self):
        risks = {r for r, cb in self.risk_boxes.items() if cb.isChecked()}
        method = self.method_box.currentText()
        fast = self.fast_box.isChecked()
        q = self.best_search.text().strip().lower()

        def ok(o):
            return (o.risk in risks and (method == "All" or o.method == method)
                    and (not fast or o.fast) and (not q or q in o.name.lower()))
        self.best_proxy.set_predicate(ok)
        n = self.best_proxy.rowCount()
        total = len(self.best_model.rows)
        self.best_count.setText(f"{n} of {total} opportunities shown · sorted by the column you click "
                                f"(default: profit per day, which rewards fast sellers)")

    def _risks_changed(self):
        self.settings.risks = [r for r, cb in self.risk_boxes.items() if cb.isChecked()]
        self._save_settings()
        self._apply_best_filter()
        self._update_plan()

    def _best_selected(self):
        o = selected_row(self.best_view)
        self.best_detail.setHtml(self.opportunity_html(o) if o else "")

    def opportunity_html(self, o) -> str:
        s = self.settings
        out = [f"<h3>{esc(o.name)} — {esc(o.method)} · {risk_badge(o.risk)}</h3>"]
        st = o.stats
        if o.method == "Craft":
            c = o.craft
            prof = c.profession
            out.append(
                f"<p><b>What to do:</b> buy the ingredients below, craft with <b>{esc(prof)}</b> "
                f"(recipe level {c.recipe.level}, your success ≈ <b>{c.success:.1%}</b>), then sell at about "
                f"<b>{kinah(o.sell_price)}</b> each.</p>")
            out.append("<table cellspacing=4><tr><th align=left>Ingredient</th><th>Qty</th><th>Unit cost</th>"
                       "<th>Total</th><th align=left>Cheapest way to get it</th></tr>")
            for line in c.lines:
                out.append(f"<tr><td>{esc(line.name)}</td><td align=right>{line.qty}</td>"
                           f"<td align=right>{kinah(line.unit)}</td><td align=right>{kinah(line.unit * line.qty)}</td>"
                           f"<td>{esc(line.source.detail)}</td></tr>")
            if c.recipe.gold:
                out.append(f"<tr><td>Crafting fee</td><td></td><td></td><td align=right>{kinah(c.recipe.gold)}</td>"
                           f"<td>kinah</td></tr>")
            out.append("</table>")
            out.append(
                f"<p>Per attempt: cost <b>{kinah(c.cost)}</b>, expected back <b>{kinah(c.ev)}</b> after "
                f"{s.tax_pct:g}% tax + {s.fee_pct:g}% listing fee → profit <b>{kinah(c.profit, True)}</b> "
                f"(if prices dip: {kinah(c.profit_pessimistic, True)}).<br>"
                f"Outcomes: fail {c.p_fail:.1%} · normal {c.p_plain:.1%}"
                + (f" · Splendent {c.p_combo:.1%} (sells ~{kinah(c.combo_value / s.keep if s.keep else 0)})"
                   if c.combo_key else "")
                + f"<br>Chance that 10 crafts lose money overall: {batch_loss_probability(c, 10):.0%}</p>")
        else:
            out.append(
                f"<p><b>What to do:</b> buy at <b>{kinah(o.buy_price)}</b> or less (cheapest listing now "
                f"{kinah(st.ask)}), then list in small lots at about <b>{kinah(o.sell_price)}</b> "
                f"(its usual price, {esc(st.ref_basis)}). Buyers usually take small amounts, so sell a few at a "
                f"time instead of one big stack.</p>"
                f"<p>Per item: profit <b>{kinah(o.profit_unit, True)}</b> after {s.tax_pct:g}% tax + "
                f"{s.fee_pct:g}% fee (if the price stays low: {kinah(o.profit_pess_unit, True)}).</p>")
        if st:
            mk = f"~{st.sales_per_day:.1f}/day" if st.sales_per_day is not None else "unknown"
            out.append(
                f"<p><b>Speed:</b> the market buys {mk} ({esc(st.sales_basis)}). As one more seller you can "
                f"expect about {MARKET_SHARE:.0%} of that: ~{o.your_per_day:.1f}/day. Suggested batch: "
                f"<b>{o.batch}</b>, sells in {speed_text(o.sell_days)}. Listings now: {st.listings}.</p>")
        if o.good:
            out.append("<p style='color:#1b8a3a'>✔ " + "<br>✔ ".join(esc(g) for g in o.good) + "</p>")
        if o.reasons:
            out.append("<p style='color:#c0392b'>⚠ " + "<br>⚠ ".join(esc(r) for r in o.reasons) + "</p>")
        return "".join(out)

    # ── tab 2: plan ──────────────────────────────────────────────────────

    def _build_plan_tab(self):
        w = QWidget()
        lay = QVBoxLayout(w)
        top = QHBoxLayout()
        top.addWidget(QLabel("Kinah I can spend:"))
        self.budget_edit = QLineEdit(short(self.settings.budget).replace(".00", ""))
        self.budget_edit.setPlaceholderText("e.g. 5m, 500k, 12000000")
        self.budget_edit.setMaximumWidth(160)
        self.budget_edit.editingFinished.connect(self._budget_changed)
        top.addWidget(self.budget_edit)
        top.addSpacing(16)
        top.addWidget(QLabel("Sell everything within:"))
        self.horizon_box = QComboBox()
        for label, _ in HORIZONS:
            self.horizon_box.addItem(label)
        self.horizon_box.setCurrentIndex(next((i for i, (_, h) in enumerate(HORIZONS) if h == self.settings.horizon), 1))
        self.horizon_box.currentIndexChanged.connect(self._horizon_changed)
        top.addWidget(self.horizon_box)
        top.addSpacing(16)
        self.plan_risk_label = QLabel()
        top.addWidget(self.plan_risk_label)
        top.addStretch(1)
        lay.addLayout(top)

        self.plan_summary = QLabel()
        self.plan_summary.setStyleSheet("font-size:15px;padding:6px")
        self.plan_summary.setWordWrap(True)
        lay.addWidget(self.plan_summary)

        cols = [
            Column("Risk", lambda l: RISK_ORDER[l.op.risk], lambda l: l.op.risk, color=lambda l: RISK_COLORS[l.op.risk],
                   tip=lambda l: "\n".join(l.op.reasons) or "No warnings"),
            Column("Do this", lambda l: l.op.method, lambda l: "Craft" if l.op.method == "Craft" else "Buy & resell"),
            Column("Sells in", lambda l: l.op.sell_days or 1e9, lambda l: speed_text(l.op.sell_days), align="right"),
            Column("Item", lambda l: l.op.name.lower(), lambda l: l.op.name, width=260),
            Column("Amount now", lambda l: l.units, lambda l: f"{l.units}" + (" crafts" if l.op.method == "Craft" else ""),
                   align="right"),
            Column("Spend now", lambda l: l.capital, lambda l: kinah(l.capital), align="right"),
            Column("Sell at", lambda l: l.op.sell_price, lambda l: kinah(l.op.sell_price), align="right"),
            Column("Expected profit", lambda l: l.profit, lambda l: kinah(l.profit, True), align="right",
                   tip=lambda l: "Over the whole period, re-buying as it sells", width=120),
        ]
        self.plan_model, self.plan_proxy, self.plan_view = make_table(cols)
        self.plan_view.sortByColumn(7, Qt.DescendingOrder)
        self.plan_view.selectionModel().selectionChanged.connect(
            lambda: self.plan_detail.setHtml(self.opportunity_html(selected_row(self.plan_view).op)
                                             if selected_row(self.plan_view) else ""))
        self.plan_detail = QTextBrowser()
        split = QSplitter(Qt.Vertical)
        split.addWidget(self.plan_view)
        split.addWidget(self.plan_detail)
        split.setSizes([330, 160])
        lay.addWidget(split, 1)

        hold = QGroupBox("How much kinah is worth holding for trading (based on what the market can absorb)")
        hl = QHBoxLayout(hold)
        self.tier_labels = []
        for name in ("Low", "Mid", "High"):
            lab = QLabel(f"<b>{name}</b><br>—")
            lab.setAlignment(Qt.AlignCenter)
            lab.setFrameShape(QFrame.StyledPanel)
            lab.setMinimumHeight(70)
            hl.addWidget(lab)
            self.tier_labels.append(lab)
        lay.addWidget(hold)
        return w

    def _budget_changed(self):
        v = parse_kinah(self.budget_edit.text())
        if v is None or v <= 0:
            self.budget_edit.setStyleSheet("border:1px solid #c0392b")
            return
        self.budget_edit.setStyleSheet("")
        self.settings.budget = v
        self._save_settings()
        self._update_plan()

    def _horizon_changed(self, i):
        self.settings.horizon = HORIZONS[i][1]
        self._save_settings()
        self._update_plan()

    def plan_ops(self):
        if not self.analysis:
            return []
        return [o for o in self.analysis.ops if o.risk in self.settings.risks]

    def _update_plan(self):
        self.plan_risk_label.setText("Using: " + (", ".join(self.settings.risks) or "nothing (tick a risk level)")
                                     + "  (change on the first tab)")
        ops = self.plan_ops()
        p = make_plan(ops, self.settings.budget, self.settings.horizon)
        self.current_plan = p
        self.plan_model.set_rows(p.lines)
        if not self.analysis:
            self.plan_summary.setText("Press Refresh to load the market.")
        elif not p.lines:
            self.plan_summary.setText("Nothing fits these settings. Tick more risk levels, raise the budget, or set "
                                      "your crafting levels in Settings.")
        else:
            self.plan_summary.setText(
                f"Spend <b>{kinah(p.capital)}</b> now → expected profit <b>{kinah(p.profit, True)}</b> over "
                f"{self.horizon_box.currentText()} (≈ <b>{kinah(p.profit_per_day)}</b>/day, "
                f"{(p.profit / p.capital if p.capital else 0):.0%} on what you spend). "
                f"Keep {kinah(p.leftover)} spare"
                + (" — the market can't absorb more of these items in this time." if p.leftover > 0.2 * p.budget
                   else "."))
        tiers = holding_tiers(ops, self.settings.horizon) if ops else []
        for lab, name in zip(self.tier_labels, ("Low", "Mid", "High")):
            t = next((t for t in tiers if t.label == name), None)
            if t:
                lab.setText(f"<b>{name}</b><br><span style='font-size:16px'>{short(t.amount)}</span><br>"
                            f"≈ {short(t.profit_per_day)} profit/day")
            else:
                lab.setText(f"<b>{name}</b><br>—")

    # ── tab 3: crafting ──────────────────────────────────────────────────

    def _build_craft_tab(self):
        w = QWidget()
        lay = QVBoxLayout(w)
        top = QHBoxLayout()
        top.addWidget(QLabel("Profession:"))
        self.prof_box = QComboBox()
        self.prof_box.addItem("All", None)
        for k, v in PROFESSIONS.items():
            self.prof_box.addItem(v.split(" (")[0], k)
        self.prof_box.currentIndexChanged.connect(self._apply_craft_filter)
        top.addWidget(self.prof_box)
        self.craft_profit_only = QCheckBox("Profitable only")
        self.craft_profit_only.setChecked(True)
        self.craft_profit_only.toggled.connect(self._apply_craft_filter)
        top.addWidget(self.craft_profit_only)
        self.craft_search = QLineEdit()
        self.craft_search.setPlaceholderText("Search recipe…")
        self.craft_search.textChanged.connect(self._apply_craft_filter)
        top.addWidget(self.craft_search, 1)
        lay.addLayout(top)
        cols = [
            Column("Item", lambda c: c.name.lower(), lambda c: c.name, width=280),
            Column("Profession", lambda c: c.profession, lambda c: c.profession.split(" (")[0], width=130),
            Column("Lvl", lambda c: c.recipe.level, align="right"),
            Column("Success", lambda c: c.success, lambda c: f"{c.success:.1%}"
                   + (" ✎" if c.recipe.id in self.settings.overrides else ""), align="right",
                   tip=lambda c: "Estimated from your level; set your in-game value in the panel below"),
            Column("Cost / craft", lambda c: c.cost, lambda c: kinah(c.cost), align="right"),
            Column("Expected back", lambda c: c.ev, lambda c: kinah(c.ev), align="right"),
            Column("Profit / craft", lambda c: c.profit, lambda c: kinah(c.profit, True), align="right",
                   color=lambda c: "#1b8a3a" if c.profit > 0 else "#c0392b"),
            Column("If prices dip", lambda c: c.profit_pessimistic, lambda c: kinah(c.profit_pessimistic, True),
                   align="right"),
            Column("Sold / day", lambda c: self._sales(c.out_key), lambda c: self._sales_text(c.out_key), align="right"),
        ]
        self.craft_model, self.craft_proxy, self.craft_view = make_table(cols)
        self.craft_view.sortByColumn(6, Qt.DescendingOrder)
        self.craft_view.selectionModel().selectionChanged.connect(self._craft_selected)

        bottom = QWidget()
        bl = QVBoxLayout(bottom)
        ov = QHBoxLayout()
        ov.addWidget(QLabel("Your in-game success % for this recipe (0 = estimate from level):"))
        self.override_spin = QDoubleSpinBox()
        self.override_spin.setRange(0, 100)
        self.override_spin.setDecimals(2)
        self.override_spin.setEnabled(False)
        self.override_spin.editingFinished.connect(self._override_changed)
        ov.addWidget(self.override_spin)
        ov.addStretch(1)
        bl.addLayout(ov)
        self.craft_detail = QTextBrowser()
        bl.addWidget(self.craft_detail)
        split = QSplitter(Qt.Vertical)
        split.addWidget(self.craft_view)
        split.addWidget(bottom)
        split.setSizes([450, 260])
        lay.addWidget(split, 1)
        self.craft_hint = QLabel()
        self.craft_hint.setWordWrap(True)
        lay.addWidget(self.craft_hint)
        return w

    def _sales(self, key):
        st = self.analysis.stats.get(key) if self.analysis else None
        return st.sales_per_day if st else None

    def _sales_text(self, key):
        v = self._sales(key)
        return "?" if v is None else f"{v:.1f}"

    def _apply_craft_filter(self):
        prof = self.prof_box.currentData()
        only = self.craft_profit_only.isChecked()
        q = self.craft_search.text().strip().lower()
        self.craft_proxy.set_predicate(lambda c: (prof is None or c.recipe.profession == prof)
                                       and (not only or c.profit > 0) and (not q or q in c.name.lower()))

    def _craft_selected(self):
        c = selected_row(self.craft_view)
        if not c:
            self.override_spin.setEnabled(False)
            self.craft_detail.setHtml("")
            return
        self.override_spin.setEnabled(True)
        self.override_spin.blockSignals(True)
        self.override_spin.setValue(self.settings.overrides.get(c.recipe.id, 0.0))
        self.override_spin.blockSignals(False)
        op = next((o for o in self.analysis.ops if o.craft is c), None) if self.analysis else None
        if op:
            self.craft_detail.setHtml(self.opportunity_html(op))
        else:
            lines = "".join(f"<tr><td>{esc(l.name)}</td><td align=right>{l.qty}</td><td align=right>{kinah(l.unit)}</td>"
                            f"<td>{esc(l.source.detail)}</td></tr>" for l in c.lines)
            self.craft_detail.setHtml(
                f"<h3>{esc(c.name)}</h3><p>Not profitable right now: cost {kinah(c.cost)} per attempt, expected back "
                f"{kinah(c.ev)} (success {c.success:.1%}).</p><table cellspacing=4>{lines}</table>")

    def _override_changed(self):
        c = selected_row(self.craft_view)
        if not c:
            return
        v = self.override_spin.value()
        if v > 0:
            self.settings.overrides[c.recipe.id] = v
        else:
            self.settings.overrides.pop(c.recipe.id, None)
        self._save_settings()
        self.schedule_recalc()

    # ── tab 4: market ────────────────────────────────────────────────────

    def _build_market_tab(self):
        w = QWidget()
        lay = QVBoxLayout(w)
        top = QHBoxLayout()
        self.market_search = QLineEdit()
        self.market_search.setPlaceholderText("Search item…")
        self.market_search.textChanged.connect(self._apply_market_filter)
        top.addWidget(self.market_search, 1)
        self.market_section = QComboBox()
        self.market_section.addItem("All sections", None)
        self.market_section.currentIndexChanged.connect(self._apply_market_filter)
        top.addWidget(self.market_section)
        self.market_reliable = QCheckBox("Hide unreliable (few listings / not selling)")
        self.market_reliable.toggled.connect(self._apply_market_filter)
        top.addWidget(self.market_reliable)
        lay.addLayout(top)
        cols = [
            Column("Item", lambda s: s.name.lower(), lambda s: s.name, width=260),
            Column("Category", lambda s: s.category or "", lambda s: s.category or "", width=150),
            Column("Cheapest now", lambda s: s.ask, lambda s: kinah(s.ask), align="right"),
            Column("Listings", lambda s: s.listings, align="right"),
            Column("Usual price", lambda s: s.ref, lambda s: kinah(s.ref), align="right",
                   tip=lambda s: s.ref_basis),
            Column("Change 24h", lambda s: s.change_pct, lambda s: "—" if s.change_pct is None else f"{s.change_pct:+.0f}%",
                   align="right"),
            Column("Trend 7d", lambda s: s.trend_7d, lambda s: pct(s.trend_7d, True), align="right"),
            Column("Sold / day", lambda s: s.sales_per_day, lambda s: "?" if s.sales_per_day is None else f"{s.sales_per_day:.1f}",
                   align="right", tip=lambda s: s.sales_basis),
            Column("Cheaper via", lambda s: self._source_text(s.key), lambda s: self._source_text(s.key), width=300,
                   tip=lambda s: "Cheaper than buying it (e.g. Substance Morph from Odyle)"),
            Column("Notes", lambda s: len(s.flags), lambda s: ", ".join(s.flags)),
        ]
        self.market_model, self.market_proxy, self.market_view = make_table(cols)
        self.market_view.sortByColumn(3, Qt.DescendingOrder)
        self.market_view.selectionModel().selectionChanged.connect(self._market_selected)
        bottom = QWidget()
        bl = QHBoxLayout(bottom)
        self.chart = PriceChart()
        bl.addWidget(self.chart, 2)
        self.market_detail = QTextBrowser()
        bl.addWidget(self.market_detail, 1)
        split = QSplitter(Qt.Vertical)
        split.addWidget(self.market_view)
        split.addWidget(bottom)
        split.setSizes([480, 200])
        lay.addWidget(split, 1)
        return w

    def _source_text(self, key):
        if not self.analysis:
            return ""
        src = self.analysis.ctx.cheapest_source(key)
        st = self.analysis.stats.get(key)
        if not src or not st or src.how in ("buy", "bulk"):
            return ""
        return f"{src.detail}: {kinah(src.cost)}" + (f" (market {kinah(st.ask)})" if st.ask else "")

    def _apply_market_filter(self):
        q = self.market_search.text().strip().lower()
        sec = self.market_section.currentData()
        rel = self.market_reliable.isChecked()
        self.market_proxy.set_predicate(lambda s: (not q or q in s.name.lower()) and (sec is None or s.section == sec)
                                        and (not rel or s.trusted))

    def _market_selected(self):
        s = selected_row(self.market_view)
        if not s or not self.analysis:
            return
        d = self.analysis.data.details.get(s.key, {})
        closes = []
        for p in sorted(d.get("hourly") or [], key=lambda p: parse_ts(p.get("ts")) or 0):
            v = p.get("close") or p.get("low")
            if isinstance(v, (int, float)):
                closes.append(float(v))
        if not closes:
            closes = [p for _, p, _ in self.analysis.data.snapshots.get(s.key, []) if p]
        self.chart.set_data(closes[-168:], s.ref)
        h = history_stats(d.get("hourly") or [])
        src = self._source_text(s.key)
        self.market_detail.setHtml(
            f"<h3>{esc(s.name)}</h3><p>Cheapest now: <b>{kinah(s.ask)}</b> ({s.listings} listings)<br>"
            f"Usual price: <b>{kinah(s.ref)}</b> <small>({esc(s.ref_basis)})</small><br>"
            f"Low end (25%): {kinah(s.pessimistic)} · swings ±{pct(s.volatility)}<br>"
            f"Sold per day: {'?' if s.sales_per_day is None else f'{s.sales_per_day:.1f}'} "
            f"<small>({esc(s.sales_basis)})</small><br>"
            + (f"Real sales (28 d): {s.units_sold_28d:,} at avg {kinah(s.sold_avg)}<br>" if s.units_sold_28d else "")
            + (f"Bulk buy now: {kinah(s.bulk_unit)} avg for {s.bulk_units} units<br>" if s.bulk_unit else "")
            + (f"<b>Cheaper:</b> {esc(src)}<br>" if src else "")
            + (f"<span style='color:#c0392b'>⚠ {esc(', '.join(s.flags))}</span>" if s.flags else "")
            + f"<br><small>{h['n']} hourly points in the last 72 h</small></p>")

    # ── tab 5: settings ──────────────────────────────────────────────────

    def _build_settings_tab(self):
        w = QWidget()
        outer = QVBoxLayout(w)
        grid = QGridLayout()
        lv = QGroupBox("Your crafting levels (proficiency). 0 = you don't have this profession.")
        form = QFormLayout(lv)
        self.level_spins = {}
        for k, label in PROFESSIONS.items():
            sp = QSpinBox()
            sp.setRange(0, 200)
            sp.setValue(int(self.settings.levels.get(k, 0)))
            sp.valueChanged.connect(self._levels_changed)
            form.addRow(label + ":", sp)
            self.level_spins[k] = sp
        grid.addWidget(lv, 0, 0)

        tx = QGroupBox("Market fees")
        tf = QFormLayout(tx)
        self.tax_spin = QDoubleSpinBox()
        self.tax_spin.setRange(0, 50)
        self.tax_spin.setSuffix(" %")
        self.tax_spin.setValue(self.settings.tax_pct)
        self.tax_spin.valueChanged.connect(self._fees_changed)
        self.fee_spin = QDoubleSpinBox()
        self.fee_spin.setRange(0, 20)
        self.fee_spin.setSuffix(" %")
        self.fee_spin.setValue(self.settings.fee_pct)
        self.fee_spin.valueChanged.connect(self._fees_changed)
        tf.addRow("Sales tax (taken when it sells):", self.tax_spin)
        tf.addRow("Listing fee (paid when you list):", self.fee_spin)
        grid.addWidget(tx, 0, 1)
        outer.addLayout(grid)

        info = QTextBrowser()
        info.setOpenExternalLinks(True)
        info.setHtml(
            "<h3>How it works</h3>"
            "<ul>"
            "<li>Prices come from the public <a href='https://aion2.exchange'>aion2.exchange</a> website (Lumiel "
            "server). The app only reads that website: it never touches the game, its files or its memory.</li>"
            "<li>Every Refresh is saved, so the app builds its own price and sales history. The more often you "
            "refresh (e.g. a few times a day), the better the sales-speed estimates get.</li>"
            "<li><b>Usual price</b> is the median of the last 72 hours of hourly prices, so one odd listing "
            "cannot fool it. Items with fewer than 3 listings, or that sit on the market without selling, are "
            "never used to claim a profit.</li>"
            "<li><b>Odyle → materials (Substance Morph):</b> every grade of Odyle can be morphed 1:1 into ore, "
            "gems, logs and plants of the same grade. Morphed materials can't be sold, so the app only uses this "
            "when Odyle is cheaper than buying the material.</li>"
            "<li>Also considered: crafting intermediates yourself (ore → ingot → gear) when cheaper than buying, "
            "alternative ingredients, NPC-shop supplies, the Splendent combo chance, failed crafts eating the "
            "materials, and buying in bulk (the cheapest listing runs out).</li>"
            "<li><b>Success rates</b> are estimated from your level using community measurements of the game's "
            "probability tab. If the game shows a different number for a recipe, type it in on the Crafting tab.</li>"
            "<li><b>Profit per day</b> is the main ranking: an item that sells in hours lets you re-use the "
            "kinah, so a small margin that sells fast can beat a big margin that sits for a week.</li>"
            "</ul>"
            f"<p><small>Data folder: {esc(self.engine.data_dir)} · Recipes: gamers4.life crafting database"
            "</small></p>")
        outer.addWidget(info, 1)
        return w

    def _levels_changed(self):
        self.settings.levels = {k: sp.value() for k, sp in self.level_spins.items()}
        self._save_settings()
        self._update_banner()
        self.schedule_recalc()

    def _fees_changed(self):
        self.settings.tax_pct = self.tax_spin.value()
        self.settings.fee_pct = self.fee_spin.value()
        self._save_settings()
        self.schedule_recalc()

    def _save_settings(self):
        try:
            self.settings.save(self.engine.settings_path)
        except OSError:
            pass

    def _update_banner(self):
        msgs = []
        if not any(self.settings.levels.values()):
            msgs.append("Set your crafting levels in the Settings tab to see crafting opportunities.")
        if self.analysis:
            msgs.extend(self.analysis.data.warnings)
            if self.analysis.data.snapshot_count < 3:
                msgs.append("Few refreshes saved yet: sales-speed estimates get better each time you press Refresh "
                            "(a few times a day is ideal).")
        self.banner.setText("\n".join(msgs))
        self.banner.setVisible(bool(msgs))

    # ── refresh / recalc ─────────────────────────────────────────────────

    def refresh(self):
        if self._thread is not None:
            return
        self.refresh_btn.setEnabled(False)
        self.progress.setValue(0)
        self.progress.show()
        self.status.setText("Refreshing…")
        self._thread = QThread(self)
        self._worker = RefreshWorker(self.engine, copy.deepcopy(self.settings))
        self._worker.moveToThread(self._thread)
        self._thread.started.connect(self._worker.run)
        self._worker.progress.connect(self._on_progress)
        self._worker.finished.connect(self._on_refreshed)
        self._worker.failed.connect(self._on_failed)
        self._worker.finished.connect(self._thread.quit)
        self._worker.failed.connect(self._thread.quit)
        self._thread.finished.connect(self._thread_done)
        self._thread.start()

    def is_refreshing(self) -> bool:
        return self._thread is not None

    @Slot(str, float)
    def _on_progress(self, msg, frac):
        self.status.setText(msg)
        self.progress.setValue(int(frac * 100))

    @Slot(object)
    def _on_refreshed(self, analysis):
        self.analysis = analysis
        self.recalculate(reuse=True)
        when = time.strftime("%H:%M", time.localtime(analysis.data.fetched_at))
        self.status.setText(("Offline copy from " if analysis.data.offline else "Updated ") + when)

    @Slot(str)
    def _on_failed(self, msg):
        self.status.setText("Refresh failed")
        QMessageBox.warning(self, "Refresh failed", msg)

    @Slot()
    def _thread_done(self):
        self.progress.hide()
        self.refresh_btn.setEnabled(True)
        self._thread.deleteLater()
        self._thread = None
        self._worker = None

    def schedule_recalc(self):
        self._recalc_timer.start()

    def recalculate(self, reuse=False):
        if not self.analysis:
            return
        if not reuse:
            self.analysis = self.engine.analyse(self.analysis.data, self.settings)
        a = self.analysis
        self.best_model.set_rows(a.ops)
        self._apply_best_filter()
        self.craft_model.set_rows(a.crafts)
        self._apply_craft_filter()
        secs = sorted({s.section for s in a.stats.values() if s.section})
        cur = self.market_section.currentData()
        self.market_section.blockSignals(True)
        self.market_section.clear()
        self.market_section.addItem("All sections", None)
        for s in secs:
            self.market_section.addItem(s.title(), s)
        idx = self.market_section.findData(cur)
        self.market_section.setCurrentIndex(max(0, idx))
        self.market_section.blockSignals(False)
        self.market_model.set_rows([s for s in a.stats.values()])
        self._apply_market_filter()
        n_levels = any(self.settings.levels.values())
        self.craft_hint.setText(
            f"{len(a.crafts)} recipes you can craft have a reliable market price."
            if n_levels else "Set your crafting levels in Settings to see recipes.")
        self._update_plan()
        self._update_banner()

    def closeEvent(self, event):
        if self._thread is not None:
            self._thread.quit()
            self._thread.wait(3000)
        super().closeEvent(event)
