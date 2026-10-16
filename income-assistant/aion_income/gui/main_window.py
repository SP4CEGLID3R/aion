"""Main window: sidebar navigation, page header with Refresh, five pages."""
from __future__ import annotations

import copy
import html
import time

from PySide6.QtCore import QObject, QSize, QStringListModel, Qt, QThread, QTimer, Signal, Slot
from PySide6.QtWidgets import (
    QCompleter, QButtonGroup, QCheckBox, QComboBox, QDoubleSpinBox, QFormLayout, QFrame, QGridLayout, QHBoxLayout, QLabel,
    QLineEdit, QMainWindow, QMessageBox, QProgressBar, QPushButton, QSpinBox, QSplitter, QStackedWidget,
    QTextBrowser, QVBoxLayout, QWidget,
)

from ..crafting import batch_loss_probability
from ..engine import Analysis, Engine, Settings
from ..opportunities import MARKET_SHARE, RISK_ORDER, speed_text
from ..planner import holding_tiers, make_adaptive_plan, stock_days_for
from ..pricing import history_stats
from ..recipes import PROFESSIONS
from .. import supply as supply_mod
from ..store import parse_ts
from . import theme
from .theme import RISK_COLORS
from .widgets import Column, KpiCard, PriceChart, kinah, make_table, parse_kinah, pct, selected_row, short

HORIZONS = [("1 day", 1.0), ("3 days", 3.0), ("7 days", 7.0), ("10 days", 10.0)]
METHOD_COLORS = {"Craft": theme.BLUE, "Resell": theme.VIOLET}

PAGES = [
    ("earn", "Best ways to earn", "Every way to make kinah on your server right now, ranked by profit per day"),
    ("plan", "My plan", "What to buy with your kinah, and how much is worth holding"),
    ("craft", "Crafting", "Every recipe you can craft, with your success rate and real costs"),
    ("market", "Market", "All listings on your server with usual prices, sales speed and history"),
    ("supply", "Supply requests", "Items your daily and weekly supply requests ask for: extra buyers while they run"),
    ("settings", "Settings", "Your crafting levels and market fees"),
]


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


def pill_html(text: str, color: str) -> str:
    return (f"<span style='background-color:{theme.tint(color, 0.16)};color:{color};font-weight:600;font-size:8.5pt'>"
            f"&nbsp;&nbsp;● {esc(text).upper()}&nbsp;&nbsp;</span>")


def stat_cell(label: str, value: str, color: str = theme.TEXT) -> str:
    return (f"<td style='padding-right:28px'><span style='color:{theme.MUTED};font-size:8pt;font-weight:600'>"
            f"{esc(label).upper()}</span><br><span style='font-size:15pt;font-weight:600;color:{color}'>"
            f"{esc(value)}</span></td>")


def card(widget_or_layout, margins=(16, 14, 16, 14)) -> QFrame:
    f = QFrame()
    f.setObjectName("Card")
    lay = QVBoxLayout(f)
    lay.setContentsMargins(*margins)
    if isinstance(widget_or_layout, QWidget):
        lay.addWidget(widget_or_layout)
    else:
        lay.addLayout(widget_or_layout)
    return f


def browser() -> QTextBrowser:
    b = QTextBrowser()
    b.setOpenExternalLinks(True)
    b.document().setDefaultStyleSheet(theme.DOC_CSS)
    return b


def muted(text: str) -> QLabel:
    lab = QLabel(text)
    lab.setObjectName("Muted")
    return lab


class MainWindow(QMainWindow):
    def __init__(self, engine: Engine, settings: Settings | None = None, auto_refresh: bool = True):
        super().__init__()
        self.engine = engine
        self.settings = settings or Settings.load(engine.settings_path)
        self.analysis: Analysis | None = None
        self.current_plan = None
        self._thread = None
        self._worker = None
        self._refresh_again = False
        self.setWindowTitle("Kinah Forge · Aion 2 Income Assistant")
        self.setWindowIcon(theme.app_icon())
        self.resize(1360, 860)
        self.setMinimumSize(1080, 680)

        root = QWidget()
        root.setObjectName("Root")
        h = QHBoxLayout(root)
        h.setContentsMargins(0, 0, 0, 0)
        h.setSpacing(0)
        h.addWidget(self._build_sidebar())

        content = QWidget()
        cl = QVBoxLayout(content)
        cl.setContentsMargins(26, 20, 26, 18)
        cl.setSpacing(12)
        cl.addLayout(self._build_header())
        self.progress = QProgressBar()
        self.progress.setRange(0, 100)
        self.progress.setTextVisible(False)
        self.progress.hide()
        cl.addWidget(self.progress)
        self.banner = QLabel()
        self.banner.setObjectName("Banner")
        self.banner.setWordWrap(True)
        self.banner.hide()
        cl.addWidget(self.banner)

        self.tabs = QStackedWidget()
        self.tabs.addWidget(self._build_best_tab())
        self.tabs.addWidget(self._build_plan_tab())
        self.tabs.addWidget(self._build_craft_tab())
        self.tabs.addWidget(self._build_market_tab())
        self.tabs.addWidget(self._build_supply_tab())
        self.tabs.addWidget(self._build_settings_tab())
        self.tabs.currentChanged.connect(self._page_changed)
        cl.addWidget(self.tabs, 1)
        h.addWidget(content, 1)
        self.setCentralWidget(root)
        self._page_changed(0)

        self._recalc_timer = QTimer(self)
        self._recalc_timer.setSingleShot(True)
        self._recalc_timer.setInterval(400)
        self._recalc_timer.timeout.connect(self.recalculate)

        self._update_banner()
        self._update_kpis()
        if auto_refresh:
            QTimer.singleShot(200, self.refresh)

    def showEvent(self, event):
        super().showEvent(event)
        theme.dark_title_bar(self)

    # ── sidebar and header ───────────────────────────────────────────────

    def _build_sidebar(self):
        side = QFrame()
        side.setObjectName("Sidebar")
        side.setFixedWidth(236)
        lay = QVBoxLayout(side)
        lay.setContentsMargins(16, 22, 16, 18)
        lay.setSpacing(4)

        brand = QHBoxLayout()
        logo = QLabel()
        pm = theme.logo_pixmap(84)
        pm.setDevicePixelRatio(2)
        logo.setPixmap(pm)
        brand.addWidget(logo)
        names = QVBoxLayout()
        names.setSpacing(0)
        t = QLabel("Kinah Forge")
        t.setObjectName("Brand")
        s = QLabel("Aion 2 Income Assistant")
        s.setObjectName("BrandSub")
        names.addWidget(t)
        names.addWidget(s)
        brand.addLayout(names, 1)
        lay.addLayout(brand)
        lay.addSpacing(26)

        sec = QLabel("MENU")
        sec.setObjectName("SectionTitle")
        lay.addWidget(sec)
        lay.addSpacing(4)
        self.nav_group = QButtonGroup(self)
        self.nav_group.setExclusive(True)
        self.nav_buttons = []
        for i, (key, title, _) in enumerate(PAGES):
            b = QPushButton("  " + title)
            b.setObjectName("Nav")
            b.setCheckable(True)
            b.setIcon(theme.nav_icon(key))
            b.setIconSize(QSize(18, 18))
            b.setCursor(Qt.PointingHandCursor)
            b.clicked.connect(lambda _=False, i=i: self.tabs.setCurrentIndex(i))
            self.nav_group.addButton(b, i)
            self.nav_buttons.append(b)
            lay.addWidget(b)
        lay.addStretch(1)

        pill = QFrame()
        pill.setObjectName("ServerPill")
        pl = QHBoxLayout(pill)
        pl.setContentsMargins(12, 10, 12, 10)
        dot = QLabel("●")
        dot.setStyleSheet(f"color:{theme.VIOLET};font-size:14pt")
        pl.addWidget(dot)
        pv = QVBoxLayout()
        pv.setSpacing(0)
        self.server_label = QLabel(self.settings.server_name.upper())
        self.server_label.setStyleSheet("font-weight:700;letter-spacing:1px")
        from ..engine import REGIONS
        self.server_sub = muted(REGIONS.get(self.settings.server_region, self.settings.server_region))
        self.server_sub.setStyleSheet(f"color:{theme.MUTED};font-size:8.5pt")
        pv.addWidget(self.server_label)
        pv.addWidget(self.server_sub)
        pl.addLayout(pv, 1)
        lay.addWidget(pill)
        lay.addSpacing(8)
        src = QLabel("Market data · aion2.exchange<br>Read-only · never touches the game")
        src.setStyleSheet(f"color:{theme.FAINT};font-size:8pt")
        lay.addWidget(src)
        return side

    def _build_header(self):
        bar = QHBoxLayout()
        titles = QVBoxLayout()
        titles.setSpacing(2)
        self.page_title = QLabel()
        self.page_title.setObjectName("PageTitle")
        self.page_subtitle = QLabel()
        self.page_subtitle.setObjectName("PageSubtitle")
        titles.addWidget(self.page_title)
        titles.addWidget(self.page_subtitle)
        bar.addLayout(titles, 1)
        self.status_dot = QLabel("●")
        self.status_dot.setStyleSheet(f"color:{theme.FAINT};font-size:11pt")
        self.status = QLabel("Not loaded yet")
        self.status.setObjectName("Muted")
        bar.addWidget(self.status_dot)
        bar.addWidget(self.status)
        bar.addSpacing(12)
        self.refresh_btn = QPushButton("  Refresh")
        self.refresh_btn.setObjectName("Primary")
        self.refresh_btn.setIcon(theme.nav_icon("refresh", "#1a1204"))
        self.refresh_btn.setIconSize(QSize(16, 16))
        self.refresh_btn.setCursor(Qt.PointingHandCursor)
        self.refresh_btn.setShortcut("F5")
        self.refresh_btn.setToolTip("Download the latest prices for your server (F5)")
        self.refresh_btn.clicked.connect(self.refresh)
        bar.addWidget(self.refresh_btn)
        return bar

    def _page_changed(self, i):
        if 0 <= i < len(PAGES):
            self.page_title.setText(PAGES[i][1])
            self.page_subtitle.setText(PAGES[i][2])
            self.nav_buttons[i].setChecked(True)
            for j, (key, _, _) in enumerate(PAGES):
                self.nav_buttons[j].setIcon(theme.nav_icon(key, theme.GOLD if j == i else theme.MUTED))

    def _set_status(self, text, color):
        self.status.setText(text)
        self.status_dot.setStyleSheet(f"color:{color};font-size:11pt")

    # ── page 1: best ways to earn ────────────────────────────────────────

    def _risk_boxes(self, row):
        boxes = {}
        tips = {
            "Safe": "Plenty of sales and stable prices; still profitable if prices drop to their recent low.",
            "Moderate": "Profitable on average with decent evidence, but could lose a little if the market dips.",
            "Risky": "Thin data, big price swings, slow selling or depends on combo luck.",
        }
        for r in ("Safe", "Moderate", "Risky"):
            cb = QCheckBox(r)
            cb.setObjectName("Chip")
            cb.setChecked(r in self.settings.risks)
            cb.setStyleSheet(f"QCheckBox#Chip {{ color:{RISK_COLORS[r]}; font-weight:600; }}"
                             f"QCheckBox#Chip::indicator:checked {{ background:{RISK_COLORS[r]};"
                             f" border:1px solid {RISK_COLORS[r]}; }}")
            cb.setToolTip(tips[r])
            cb.setCursor(Qt.PointingHandCursor)
            cb.toggled.connect(self._risks_changed)
            boxes[r] = cb
            row.addWidget(cb)
        return boxes

    def _build_best_tab(self):
        w = QWidget()
        lay = QVBoxLayout(w)
        lay.setContentsMargins(0, 4, 0, 0)
        lay.setSpacing(12)
        kpis = QHBoxLayout()
        kpis.setSpacing(12)
        self.kpi_best = KpiCard("Top profit / day", theme.GOLD)
        self.kpi_safe = KpiCard("Safe options", theme.GREEN)
        self.kpi_fast = KpiCard("Fast sellers", theme.VIOLET)
        self.kpi_items = KpiCard("Items tracked", theme.TEXT)
        for k in (self.kpi_best, self.kpi_safe, self.kpi_fast, self.kpi_items):
            kpis.addWidget(k)
        lay.addLayout(kpis)
        self.weekly_label = QLabel("WEEKLY CYCLE  ·  " + supply_mod.weekly_tip())
        self.weekly_label.setWordWrap(True)
        self.weekly_label.setStyleSheet(f"color:{theme.VIOLET};padding:2px 2px")
        self.weekly_label.setToolTip("From price history around the weekly reset (supply requests) on EU and NA servers; only what held in both "
                                     "regions is shown. One week of data, so treat it as a hint.")
        lay.addWidget(self.weekly_label)

        filt = QHBoxLayout()
        filt.setSpacing(8)
        self.risk_boxes = self._risk_boxes(filt)
        filt.addSpacing(10)
        self.method_box = QComboBox()
        self.method_box.addItems(["All methods", "Craft", "Resell"])
        self.method_box.currentIndexChanged.connect(self._apply_best_filter)
        filt.addWidget(self.method_box)
        self.fast_box = QCheckBox("⚡ Fast sellers only")
        self.fast_box.setObjectName("Chip")
        self.fast_box.setToolTip("Only things that sell within about a day")
        self.fast_box.setCursor(Qt.PointingHandCursor)
        self.fast_box.toggled.connect(self._apply_best_filter)
        filt.addWidget(self.fast_box)
        self.shop_btn = QPushButton("🛒  Shopping list")
        self.shop_btn.setToolTip("Tick items in the first column, then get every material you need to buy for them, "
                                 "with quantities and prices")
        self.shop_btn.setCursor(Qt.PointingHandCursor)
        self.shop_btn.clicked.connect(self.open_shopping_list)
        filt.addWidget(self.shop_btn)
        filt.addStretch(1)
        self.best_search = QLineEdit()
        self.best_search.setPlaceholderText("Search item…")
        self.best_search.setClearButtonEnabled(True)
        self.best_search.setFixedWidth(260)
        self.best_search.textChanged.connect(self._apply_best_filter)
        filt.addWidget(self.best_search)
        lay.addLayout(filt)

        cols = [
            Column("", lambda o: 0, lambda o: "", check=lambda o: (o.method, o.key), width=40,
                   tip=lambda o: "Tick to add it to the shopping list"),
            Column("Risk", lambda o: RISK_ORDER[o.risk], lambda o: o.risk, color=lambda o: RISK_COLORS[o.risk],
                   tip=lambda o: "\n".join(o.reasons) or "No warnings", pill=True, width=118),
            Column("Method", lambda o: o.method, color=lambda o: METHOD_COLORS[o.method], pill=True, width=112),
            Column("Item", lambda o: o.name.lower(), lambda o: o.name, width=270, bold=True),
            Column("Profit / day", lambda o: o.real_profit_day, lambda o: kinah(o.real_profit_day), align="right",
                   color=lambda o: theme.GOLD, bold=True, width=120,
                   tip=lambda o: f"Realistic profit per day (predicted {kinah(o.profit_day)}): trades like this "
                                 f"really made about this much in simulations on real price history"),
            Column("Profit / item", lambda o: o.real_profit_unit, lambda o: kinah(o.real_profit_unit), align="right",
                   color=lambda o: theme.GREEN, width=115,
                   tip=lambda o: f"Realistic, per item bought / craft attempt, after tax and fee "
                                 f"(predicted {kinah(o.profit_unit)})"),
            Column("ROI", lambda o: o.real_roi, lambda o: pct(o.real_roi), align="right", width=70,
                   tip=lambda o: f"Realistic return (predicted {pct(o.roi)})"),
            Column("Sells in", lambda o: o.sell_days if o.sell_days is not None else 1e9,
                   lambda o: speed_text(o.sell_days), align="right", width=95,
                   color=lambda o: theme.VIOLET if o.fast else None,
                   tip=lambda o: "Time for a one-day batch to sell at the suggested price"),
            Column("Buy now", lambda o: o.batch, lambda o: f"{o.batch}", align="right", width=85,
                   tip=lambda o: "Suggested amount: about one day of what you can sell"),
            Column("Per listing", lambda o: o.profit_per_listing, lambda o: kinah(o.profit_per_listing), align="right",
                   width=110, color=lambda o: theme.GOLD,
                   tip=lambda o: f"Profit per listing you put up (lots of {o.lot}). You can only have 10 listings at "
                                 f"once and each takes ~5 min, so this is what your time is worth on this item."),
            Column("Cost / item", lambda o: o.cost_unit, lambda o: kinah(o.cost_unit), align="right"),
            Column("Sell at", lambda o: o.sell_price, lambda o: kinah(o.sell_price), align="right"),
            Column("Notes", lambda o: len(o.reasons),
                   lambda o: "; ".join(o.reasons[:2]) if o.reasons else "; ".join(o.good[:1]),
                   color=lambda o: theme.MUTED),
        ]
        self.best_model, self.best_proxy, self.best_view = make_table(cols)
        self.best_model.checked_changed.connect(self._update_shop_btn)
        self.best_view.sortByColumn(4, Qt.DescendingOrder)
        self.best_view.selectionModel().selectionChanged.connect(self._best_selected)
        self.best_detail = browser()
        self.best_detail.setHtml(self._empty_detail("Select a row to see exactly what to do."))
        split = QSplitter(Qt.Vertical)
        split.addWidget(self.best_view)
        split.addWidget(self.best_detail)
        split.setSizes([330, 400])  # room to read the instructions and warnings
        split.setChildrenCollapsible(False)
        lay.addWidget(split, 1)
        self.best_count = muted("")
        lay.addWidget(self.best_count)
        return w

    def _update_shop_btn(self):
        n = len(self.best_model.checked_rows())
        self.shop_btn.setText(f"🛒  Shopping list ({n})" if n else "🛒  Shopping list")

    def open_shopping_list(self):
        from .shopping_dialog import ShoppingDialog
        ops = self.best_model.checked_rows()
        if not self.analysis or not ops:
            QMessageBox.information(self, "Shopping list", "Tick one or more items in the first column of the "
                                                           "table, then press Shopping list.")
            return None
        dlg = ShoppingDialog(self.analysis.ctx, ops, self)
        dlg.show()
        self._shopping = dlg  # keep it alive while open
        return dlg

    def _empty_detail(self, text):
        return f"<p class='muted' style='margin-top:12px'>{esc(text)}</p>"

    def _apply_best_filter(self):
        risks = {r for r, cb in self.risk_boxes.items() if cb.isChecked()}
        method = self.method_box.currentText()
        fast = self.fast_box.isChecked()
        q = self.best_search.text().strip().lower()

        def ok(o):
            return (o.risk in risks and (method == "All methods" or o.method == method)
                    and (not fast or o.fast) and (not q or q in o.name.lower()))
        self.best_proxy.set_predicate(ok)
        n = self.best_proxy.rowCount()
        total = len(self.best_model.rows)
        self.best_count.setText(f"{n} of {total} opportunities · click a column to sort · default sort is profit "
                                f"per day, which favours fast sellers")

    def _risks_changed(self):
        self.settings.risks = [r for r, cb in self.risk_boxes.items() if cb.isChecked()]
        self._save_settings()
        self._apply_best_filter()
        self._update_plan()
        self._update_kpis()

    def _best_selected(self):
        o = selected_row(self.best_view)
        self.best_detail.setHtml(self.opportunity_html(o) if o else self._empty_detail("Select a row."))

    def splendent_html(self, sp) -> str:
        """How to get a Splendent item and what one costs from scratch."""
        p, c = sp.parent, sp
        out = [f"<h2>{esc(sp.name)}</h2>",
               f"<p>{pill_html('Splendent', theme.GOLD)}&nbsp;&nbsp;<span class='muted'>from combo procs when crafting "
               f"{esc(p.name)} · {esc(p.profession.split(' (')[0])} · recipe lv {p.recipe.level}</span></p>",
               "<table cellspacing=0><tr>",
               stat_cell("Cost / piece", kinah(sp.cost), theme.TEXT),
               stat_cell("Sells at", kinah(sp.sell) if sp.sell else "—", theme.TEXT),
               stat_cell("Gain / piece", kinah(sp.profit, True) if sp.priced else "—",
                         theme.GREEN if sp.profit > 0 else theme.RED),
               stat_cell("Chance / attempt", f"{sp.success:.1%}", theme.GOLD),
               stat_cell("Sold / day", self._sales_text(sp.out_key)),
               "</tr></table>"]
        if sp.notes:
            out.append("<p><span class='warnhead'>WATCH OUT</span><br>"
                       + "<br>".join(f"<span class='warn'>⚠ {esc(n)}</span>" for n in sp.notes) + "</p>")
        out.append(
            f"<p><b>How to get it:</b> craft <b>{esc(p.name)}</b> (your success ≈ {p.success:.1%}). About "
            f"<span class='gold'>{sp.success:.1%}</span> of attempts (≈ 1 in {sp.attempts_per_piece:.0f}) proc "
            f"<b>{esc(sp.name)}</b>. One piece costs about <b>{kinah(sp.cost)}</b> from scratch: every ingredient "
            f"bought or made the cheapest way, failed attempts included, and the normal {esc(p.name)} you make on "
            f"the way sold at their low-end price"
            + (f". It sells for about {kinah(sp.sell)} ({kinah(sp.net)} after tax)." if sp.sell else ".")
            + "</p>")
        out.append("<table cellspacing=0><tr><th>INGREDIENT (PER PIECE)</th><th align=right>QTY</th>"
                   "<th align=right>UNIT</th><th align=right>TOTAL</th><th>CHEAPEST SOURCE</th></tr>")
        for line in sp.lines:
            out.append(f"<tr><td>{esc(line.name)}</td><td align=right>{line.qty:,.1f}</td>"
                       f"<td align=right>{kinah(line.unit)}</td><td align=right>{kinah(line.unit * line.qty)}</td>"
                       f"<td class='muted'>{esc(line.source.detail)}</td></tr>")
        out.append("</table>")
        if c.parent.combo_best:
            out.append(f"<p class='muted'>Its profit is also counted in {esc(p.name)}'s row (Best ways to earn) "
                       f"when it has real buyers.</p>")
        return "".join(out)

    def craft_extra_html(self, c) -> str:
        """Average cost per finished item, and what to do with Splendent pieces from combo procs."""
        out = [f"<p><span class='muted'>Average cost per finished item (failures included):</span> "
               f"<b>{kinah(c.avg_cost_item)}</b></p>"]
        if c.combo_name:
            proc = c.success * c.recipe.combo_chance * (c.combo_rate or 0)
            head = (f"<b>Combo procs:</b> about {proc:.1%} of attempts (≈ 1 in {1 / proc:.0f}) give "
                    f"<b>{esc(c.combo_name)}</b>. " if proc > 0 else f"<b>Combo procs</b> give <b>{esc(c.combo_name)}</b>. ")
            if c.combo_best == "craft":
                tail = (f"<span class='green'>Use them:</span> crafting <b>{esc(c.combo_use_name)}</b> with one is worth "
                        f"about <b>{kinah(c.combo_use)}</b> per piece (after that craft's other ingredients, its "
                        f"failures and tax)" + (f", vs {kinah(c.combo_sell)} if you sell it." if c.combo_sell else
                                                 ", and nobody is buying it on the market right now."))
            elif c.combo_best == "sell":
                tail = (f"<span class='green'>Sell them:</span> about <b>{kinah(c.combo_sell)}</b> each after tax"
                        + (f" (as the base for {esc(c.combo_use_name)} one is worth only {kinah(c.combo_use)})."
                           if c.combo_use else "."))
            else:
                tail = ("<span class='amber'>Keep them for now:</span> nobody is buying them and no next-tier craft "
                        "you can make turns them into profit at today's prices, so they're counted as normal items.")
            out.append(f"<p>{head}{tail}</p>")
        return "".join(out)

    def opportunity_html(self, o) -> str:
        s = self.settings
        st = o.stats
        sub = "buy low, sell at the usual price"
        if o.method == "Craft":
            sub = f"{o.craft.profession.split(' (')[0]} · recipe lv {o.craft.recipe.level}"
        out = [f"<h2>{esc(o.name)}</h2>",
               f"<p>{pill_html(o.risk, RISK_COLORS[o.risk])}&nbsp;&nbsp;{pill_html(o.method, METHOD_COLORS[o.method])}"
               f"&nbsp;&nbsp;<span class='muted'>{esc(sub)}</span></p>",
               "<table cellspacing=0><tr>",
               stat_cell("Profit / day", kinah(o.real_profit_day), theme.GOLD),
               stat_cell("Profit / item", kinah(o.real_profit_unit, True), theme.GREEN),
               stat_cell("Return", pct(o.real_roi, True), theme.GREEN),
               stat_cell("Sells in", speed_text(o.sell_days), theme.VIOLET if o.fast else theme.TEXT),
               stat_cell("Buy now", str(o.batch)),
               stat_cell("Sell at", kinah(o.sell_price)),
               "</tr></table>"]
        if o.reasons:
            out.append(f"<p style='margin-top:8px'><span class='warnhead'>WHY IT'S {o.risk.upper()} — WATCH OUT</span><br>"
                       + "<br>".join(f"<span class='warn'>⚠ {esc(r)}</span>" for r in o.reasons) + "</p>")
        if o.method == "Craft":
            c = o.craft
            out.append(
                f"<p><b>What to do:</b> buy the ingredients below, craft with <b>{esc(c.profession)}</b> "
                f"(your success ≈ <span class='gold'>{c.success:.1%}</span>), then sell at about "
                f"<span class='gold'>{kinah(o.sell_price)}</span> each.</p>")
            out.append("<table cellspacing=0><tr><th>INGREDIENT</th><th align=right>QTY</th>"
                       "<th align=right>UNIT</th><th align=right>TOTAL</th><th>CHEAPEST SOURCE</th></tr>")
            for line in c.lines:
                src_cls = "gold" if line.source.how == "morph" else ("amber" if "estimated" in line.source.detail
                                                                     else "muted")
                out.append(f"<tr><td>{esc(line.name)}</td><td align=right>{line.qty}</td>"
                           f"<td align=right>{kinah(line.unit)}</td><td align=right>{kinah(line.unit * line.qty)}</td>"
                           f"<td class='{src_cls}'>{esc(line.source.detail)}</td></tr>")
            if c.recipe.gold:
                out.append(f"<tr><td>Crafting fee</td><td></td><td></td><td align=right>{kinah(c.recipe.gold)}</td>"
                           f"<td class='muted'>kinah</td></tr>")
            out.append("</table>")
            out.append(
                f"<p>Per attempt: cost <b>{kinah(c.cost)}</b> → expected back <b>{kinah(c.ev)}</b> after "
                f"{s.tax_pct:g}% tax + {s.fee_pct:g}% fee = <span class='green'>{kinah(c.profit, True)}</span> "
                f"<span class='muted'>(if prices dip: {kinah(c.profit_pessimistic, True)})</span><br>"
                f"<span class='muted'>Outcomes:</span> fail {c.p_fail:.1%} · normal {c.p_plain:.1%}"
                + (f" · Splendent {c.p_combo:.1%}" if c.combo_key else "")
                + f" · <span class='muted'>chance 10 crafts lose money:</span> {batch_loss_probability(c, 10):.0%}</p>")
            out.append(self.craft_extra_html(c))
        else:
            out.append(
                f"<p><b>What to do:</b> buy at <span class='gold'>{kinah(o.buy_price)}</span> or less (cheapest "
                f"listing now {kinah(st.ask)}), then list in small lots at about "
                f"<span class='gold'>{kinah(o.sell_price)}</span> — its usual price ({esc(st.ref_basis)}). Buyers "
                f"usually take small amounts, so sell a few at a time instead of one big stack.</p>"
                f"<p>Per item: <span class='green'>{kinah(o.profit_unit, True)}</span> after {s.tax_pct:g}% tax + "
                f"{s.fee_pct:g}% fee <span class='muted'>(if the price stays low: "
                f"{kinah(o.profit_pess_unit, True)})</span></p>")
        if st:
            mk = f"~{st.sales_per_day:.1f}/day" if st.sales_per_day is not None else "unknown"
            out.append(
                f"<p><span class='muted'>Speed:</span> the market buys {mk} ({esc(st.sales_basis)}); as one more "
                f"seller you can expect ~{MARKET_SHARE:.0%} of that, ~{o.your_per_day:.1f}/day. "
                f"Listings now: {st.listings}.<br><span class='muted'>Listing:</span> lots of <b>{o.lot}</b> "
                f"(buyers take whole listings and prefer small ones) · ~{o.listings_per_day:.1f} listings/day · "
                f"~{o.minutes_per_day:.0f} min/day of your time · <span class='gold'>{kinah(o.profit_per_listing)}</span> "
                f"per listing</p>")
        out.append(f"<p class='muted'>Realistic numbers above. The raw prediction is {kinah(o.profit_unit, True)} per item "
                   f"({pct(o.roi, True)}); in simulations on EU servers' real prices, trades predicted like this "
                   f"made about {pct(o.real_roi, True)}. New-game prices are hard to predict, so the plan spreads "
                   f"your kinah (max {self.settings.max_share:.0%} per item, set in Settings).</p>")
        if o.good:
            out.append("<p class='green'>✔ " + "<br>✔ ".join(esc(g) for g in o.good) + "</p>")
        return "".join(out)

    # ── page 2: plan ─────────────────────────────────────────────────────

    def _build_plan_tab(self):
        w = QWidget()
        lay = QVBoxLayout(w)
        lay.setContentsMargins(0, 4, 0, 0)
        lay.setSpacing(12)

        top = QHBoxLayout()
        top.setSpacing(10)
        top.addWidget(muted("KINAH TO SPEND"))
        self.budget_edit = QLineEdit(short(self.settings.budget).replace(".00", ""))
        self.budget_edit.setPlaceholderText("e.g. 5m, 500k")
        self.budget_edit.setFixedWidth(150)
        self.budget_edit.editingFinished.connect(self._budget_changed)
        top.addWidget(self.budget_edit)
        top.addSpacing(14)
        top.addWidget(muted("SELL WITHIN"))
        self.horizon_box = QComboBox()
        for label, _ in HORIZONS:
            self.horizon_box.addItem(label)
        idx = next((i for i, (_, hh) in enumerate(HORIZONS) if hh == self.settings.horizon), None)
        if idx is None:  # a saved value that isn't one of the choices: use the default 10 days
            idx = len(HORIZONS) - 1
            self.settings.horizon = HORIZONS[idx][1]
        self.horizon_box.setCurrentIndex(idx)
        self.horizon_box.currentIndexChanged.connect(self._horizon_changed)
        top.addWidget(self.horizon_box)
        top.addStretch(1)
        self.plan_risk_label = muted("")
        top.addWidget(self.plan_risk_label)
        lay.addLayout(top)

        kpis = QHBoxLayout()
        kpis.setSpacing(12)
        self.kpi_spend = KpiCard("Spend now", theme.TEXT)
        self.kpi_profit = KpiCard("Expected profit", theme.GREEN)
        self.kpi_day = KpiCard("Profit per day", theme.GOLD)
        self.kpi_spare = KpiCard("Kept spare", theme.MUTED)
        for k in (self.kpi_spend, self.kpi_profit, self.kpi_day, self.kpi_spare):
            kpis.addWidget(k)
        lay.addLayout(kpis)

        self.plan_summary = muted("")
        self.plan_summary.setWordWrap(True)
        lay.addWidget(self.plan_summary)

        cols = [
            Column("Risk", lambda l: RISK_ORDER[l.op.risk], lambda l: l.op.risk, color=lambda l: RISK_COLORS[l.op.risk],
                   tip=lambda l: "\n".join(l.op.reasons) or "No warnings", pill=True, width=118),
            Column("Do this", lambda l: l.op.method, lambda l: "Craft" if l.op.method == "Craft" else "Resell",
                   color=lambda l: METHOD_COLORS[l.op.method], pill=True, width=112),
            Column("Sells in", lambda l: l.op.sell_days or 1e9, lambda l: speed_text(l.op.sell_days), align="right",
                   width=90),
            Column("Item", lambda l: l.op.name.lower(), lambda l: l.op.name, width=270, bold=True),
            Column("Amount now", lambda l: l.units, lambda l: f"{l.units}" + (" crafts" if l.op.method == "Craft" else ""),
                   align="right", tip=lambda l: "Up to a few days of sales at once: good chances don't show up every day, "
                                                "so holding stock keeps you selling"),
            Column("Spend now", lambda l: l.capital, lambda l: kinah(l.capital), align="right"),
            Column("List as", lambda l: l.op.lot, lambda l: f"lots of {l.op.lot}", align="right", width=90),
            Column("Sell at", lambda l: l.op.sell_price, lambda l: kinah(l.op.sell_price), align="right"),
            Column("Expected profit", lambda l: l.profit, lambda l: kinah(l.profit, True), align="right",
                   color=lambda l: theme.GREEN, bold=True,
                   tip=lambda l: "Over the whole period, re-buying as it sells", width=130),
        ]
        self.plan_model, self.plan_proxy, self.plan_view = make_table(cols)
        self.plan_view.sortByColumn(8, Qt.DescendingOrder)
        self.plan_view.selectionModel().selectionChanged.connect(
            lambda: self.plan_detail.setHtml(self.opportunity_html(selected_row(self.plan_view).op)
                                             if selected_row(self.plan_view) else ""))
        self.plan_detail = browser()
        self.plan_detail.setHtml(self._empty_detail("Select a row to see exactly what to do."))
        split = QSplitter(Qt.Vertical)
        split.addWidget(self.plan_view)
        split.addWidget(self.plan_detail)
        split.setSizes([300, 170])
        split.setChildrenCollapsible(False)
        lay.addWidget(split, 1)

        sec = QLabel("HOW MUCH KINAH IS WORTH HOLDING FOR TRADING")
        sec.setObjectName("SectionTitle")
        lay.addWidget(sec)
        tiers = QHBoxLayout()
        tiers.setSpacing(12)
        self.tier_labels = []
        for name in ("Low", "Mid", "High"):
            f = QFrame()
            f.setObjectName("TierCard")
            f.setProperty("accent", name == "Mid")
            fl = QVBoxLayout(f)
            fl.setContentsMargins(16, 12, 16, 12)
            lab = QLabel(f"<b>{name}</b><br>—")
            lab.setTextFormat(Qt.RichText)
            fl.addWidget(lab)
            tiers.addWidget(f)
            self.tier_labels.append(lab)
        lay.addLayout(tiers)
        return w

    def _budget_changed(self):
        v = parse_kinah(self.budget_edit.text())
        if v is None or v <= 0:
            self.budget_edit.setProperty("invalid", True)
            self.budget_edit.style().polish(self.budget_edit)
            return
        self.budget_edit.setProperty("invalid", False)
        self.budget_edit.style().polish(self.budget_edit)
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
        self.plan_risk_label.setText("Risk levels: " + (", ".join(self.settings.risks) or "none ticked")
                                     + " · change them on Best ways to earn")
        ops = self.plan_ops()
        drift = self.analysis.data.calibration.get("drift") if self.analysis else None
        p, stock_days, trend = make_adaptive_plan(ops, self.settings.budget, self.settings.horizon,
                                                  self.settings.play_hours, drift, self.settings.max_share)
        self.current_plan, self.stock_days = p, stock_days
        if stock_days < stock_days_for(trend) and stock_days >= self.settings.horizon:
            stock_note = f" Buying at most {stock_days:g} days of stock, as set by Sell within."
        elif trend is None:
            stock_note = (f" Buying at most {stock_days:g} days of stock: not enough price history yet to tell "
                          f"whether prices are rising or falling.")
        else:
            why = ("prices are holding" if trend >= -0.005 else "prices are falling, so stored stock would lose value")
            stock_note = (f" These items moved {trend:+.1%}/day this week: {why}, so buy at most {stock_days:g} "
                          f"days of stock.")
        self.plan_model.set_rows(p.lines)
        if not self.analysis:
            self.plan_summary.setText("Press Refresh to load the market.")
            for k in (self.kpi_spend, self.kpi_profit, self.kpi_day, self.kpi_spare):
                k.set("—")
        elif not p.lines:
            self.plan_summary.setText("Nothing fits these settings. Tick more risk levels, raise the budget, or set "
                                      "your crafting levels in Settings.")
            for k in (self.kpi_spend, self.kpi_profit, self.kpi_day, self.kpi_spare):
                k.set("—")
        else:
            roi = p.profit / p.capital if p.capital else 0
            self.kpi_spend.set(short(p.capital), f"of {short(p.budget)} budget")
            self.kpi_profit.set("+" + short(p.profit), f"over {self.horizon_box.currentText()} · {roi:.0%} return")
            self.kpi_day.set("+" + short(p.profit_per_day), "re-buying as items sell")
            self.kpi_spare.set(short(p.leftover), f"{len(p.lines)}/10 listing slots · {p.minutes / 60:.1f} of "
                               f"{self.settings.play_hours:g} h/day busy")
            self.plan_summary.setText(
                f"Spend {kinah(p.capital)} now → expected profit {kinah(p.profit, True)} over "
                f"{self.horizon_box.currentText()} (≈ {kinah(p.profit_per_day)}/day). Keep {kinah(p.leftover)} spare"
                + (" — with 10 listing slots, ~5 min per listing and your playing time, the market can't take more."
                   if p.leftover > 0.2 * p.budget else ".") + stock_note)
        tiers = (holding_tiers(ops, self.settings.horizon, self.settings.play_hours, stock_days, self.settings.max_share)
                 if ops else [])
        for lab, name in zip(self.tier_labels, ("Low", "Mid", "High")):
            t = next((t for t in tiers if t.label == name), None)
            color = theme.GOLD if name == "Mid" else theme.TEXT
            if t:
                lab.setText(f"<span style='color:{theme.MUTED};font-size:8.5pt;font-weight:600'>{name.upper()}"
                            f"{' · RECOMMENDED' if name == 'Mid' else ''}</span><br>"
                            f"<span style='font-size:18pt;font-weight:650;color:{color}'>{short(t.amount)}</span><br>"
                            f"<span style='color:{theme.GREEN}'>≈ +{short(t.profit_per_day)} / day</span>")
            else:
                lab.setText(f"<span style='color:{theme.MUTED};font-size:8.5pt;font-weight:600'>{name.upper()}</span>"
                            f"<br><span style='font-size:18pt;color:{theme.FAINT}'>—</span>"
                            + (f"<br><span style='color:{theme.FAINT}'>more kinah earns no more here</span>" if tiers else ""))

    # ── page 3: crafting ─────────────────────────────────────────────────

    def _build_craft_tab(self):
        w = QWidget()
        lay = QVBoxLayout(w)
        lay.setContentsMargins(0, 4, 0, 0)
        lay.setSpacing(12)
        top = QHBoxLayout()
        top.setSpacing(8)
        self.prof_box = QComboBox()
        self.prof_box.addItem("All professions", None)
        for k, v in PROFESSIONS.items():
            self.prof_box.addItem(v.split(" (")[0], k)
        self.prof_box.currentIndexChanged.connect(self._apply_craft_filter)
        top.addWidget(self.prof_box)
        self.craft_profit_only = QCheckBox("Profitable only")
        self.craft_profit_only.setObjectName("Chip")
        self.craft_profit_only.setChecked(False)  # show every recipe you can make by default
        self.craft_profit_only.toggled.connect(self._apply_craft_filter)
        top.addWidget(self.craft_profit_only)
        top.addStretch(1)
        self.craft_search = QLineEdit()
        self.craft_search.setPlaceholderText("Search recipe…")
        self.craft_search.setClearButtonEnabled(True)
        self.craft_search.setFixedWidth(260)
        self.craft_search.textChanged.connect(self._apply_craft_filter)
        top.addWidget(self.craft_search)
        lay.addLayout(top)
        cols = [
            Column("Item", lambda c: c.name.lower(), lambda c: ("✦ " + c.name) if getattr(c, "is_splendent", False) else c.name,
                   width=320, bold=True, color=lambda c: theme.GOLD if getattr(c, "is_splendent", False) else None,
                   tip=lambda c: (f"Splendent: you get it from combo procs when crafting {c.parent.name}"
                                  if getattr(c, "is_splendent", False) else "")),
            Column("Profession", lambda c: c.profession, lambda c: c.profession.split(" (")[0], width=140,
                   color=lambda c: theme.MUTED),
            Column("Lvl", lambda c: c.recipe.level, align="right", width=60),
            Column("Success", lambda c: c.success, lambda c: f"{c.success:.1%}"
                   + (" ✎" if c.recipe.id in self.settings.overrides else ""), align="right", width=95,
                   color=lambda c: theme.VIOLET if c.recipe.id in self.settings.overrides else None,
                   tip=lambda c: (f"Chance per {c.parent.name} attempt to get this Splendent (≈ 1 in "
                                  f"{c.attempts_per_piece:.0f})" if getattr(c, "is_splendent", False)
                                  else "Estimated from your level; set your in-game value below")),
            Column("Cost / craft", lambda c: c.cost, lambda c: kinah(c.cost), align="right"),
            Column("Avg cost / item", lambda c: c.avg_cost_item, lambda c: kinah(c.avg_cost_item), align="right",
                   width=130, tip=lambda c: "Average kinah per finished item, failed attempts included"),
            Column("Expected back", lambda c: c.ev, lambda c: kinah(c.ev) if c.priced else "no price", align="right",
                   width=120),
            Column("Profit / craft", lambda c: c.profit if c.priced else float("-inf"),
                   lambda c: (kinah(c.profit, True) + (" *" if getattr(c, "missing", None) else "")) if c.priced else "—",
                   align="right", bold=True,
                   color=lambda c: theme.GREEN if c.profit > 0 else theme.RED, width=120),
            Column("If prices dip", lambda c: c.profit_pessimistic if c.priced else float("-inf"),
                   lambda c: kinah(c.profit_pessimistic, True) if c.priced else "—",
                   align="right", color=lambda c: theme.MUTED, width=115),
            Column("Sold / day", lambda c: self._sales(c.out_key), lambda c: self._sales_text(c.out_key), align="right"),
            Column("Cost / Splendent", lambda c: c.combo_cost, lambda c: kinah(c.combo_cost) if c.combo_cost else "",
                   align="right", width=130,
                   tip=lambda c: (f"Average kinah to get one {c.combo_name} by crafting this from scratch (every "
                                  f"ingredient bought or made the cheapest way, failures included, normal results "
                                  f"sold)" if c.combo_name else "")),
            Column("Splendent sells", lambda c: c.combo_price, lambda c: kinah(c.combo_price) if c.combo_price else "",
                   align="right", width=125,
                   color=self._splendent_color,
                   tip=lambda c: "Cheapest listing / usual price of the Splendent on the market. Green: sells for "
                                 "more than it costs to make (after tax) and people really buy it. Amber: listed "
                                 "above its cost, but no sales record yet, so that price may only be an asking price."),
            Column("Splendent (combo)", lambda c: c.combo_value, self._splendent_text, width=230,
                   color=lambda c: theme.GREEN if c.combo_best == "craft" else (theme.GOLD if c.combo_best else theme.MUTED),
                   tip=lambda c: (f"Combo procs give {c.combo_name}. Click the recipe for the full advice."
                                  if c.combo_name else "This recipe has no Splendent version")),
        ]
        self.craft_model, self.craft_proxy, self.craft_view = make_table(cols)
        self.craft_view.sortByColumn(7, Qt.DescendingOrder)
        self.craft_view.selectionModel().selectionChanged.connect(self._craft_selected)

        bottom = QWidget()
        bl = QVBoxLayout(bottom)
        bl.setContentsMargins(0, 0, 0, 0)
        bl.setSpacing(8)
        ov = QHBoxLayout()
        ov.addWidget(muted("Your in-game success % for this recipe (0 = estimate from your level)"))
        self.override_spin = QDoubleSpinBox()
        self.override_spin.setRange(0, 100)
        self.override_spin.setDecimals(2)
        self.override_spin.setSuffix(" %")
        self.override_spin.setFixedWidth(120)
        self.override_spin.setEnabled(False)
        self.override_spin.editingFinished.connect(self._override_changed)
        ov.addWidget(self.override_spin)
        ov.addStretch(1)
        bl.addLayout(ov)
        self.craft_detail = browser()
        self.craft_detail.setHtml(self._empty_detail("Select a recipe to see its ingredients and where to get "
                                                     "each one cheapest."))
        bl.addWidget(self.craft_detail)
        split = QSplitter(Qt.Vertical)
        split.addWidget(self.craft_view)
        split.addWidget(bottom)
        split.setSizes([380, 360])
        split.setChildrenCollapsible(False)
        lay.addWidget(split, 1)
        self.craft_hint = muted("")
        self.craft_hint.setWordWrap(True)
        lay.addWidget(self.craft_hint)
        return w

    def _splendent_color(self, c):
        if not (c.combo_price and c.combo_cost) or c.combo_price * self.settings.keep <= c.combo_cost:
            return None
        st = self.analysis.stats.get(c.recipe.combo) if self.analysis else None
        return theme.GREEN if st and (st.sales_per_day or 0) > 0 else theme.AMBER

    @staticmethod
    def _splendent_text(c) -> str:
        if not c.combo_name:
            return ""
        if c.combo_best == "craft":
            return f"use for {c.combo_use_name} (~{short(c.combo_use)})"
        if c.combo_best == "sell":
            return f"sell (~{short(c.combo_sell)} each)"
        return "keep (no buyers or use yet)"

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
            self.craft_detail.setHtml(self._empty_detail("Select a recipe."))
            return
        self.override_spin.setEnabled(True)
        self.override_spin.blockSignals(True)
        self.override_spin.setValue(self.settings.overrides.get(c.recipe.id, 0.0))
        self.override_spin.blockSignals(False)
        if getattr(c, "is_splendent", False):
            self.craft_detail.setHtml(self.splendent_html(c))
            return
        op = next((o for o in self.analysis.ops if o.craft is c), None) if self.analysis else None
        if op:
            self.craft_detail.setHtml(self.opportunity_html(op))
        else:
            lines = "".join(f"<tr><td>{esc(l.name)}</td><td align=right>{l.qty}</td><td align=right>{kinah(l.unit)}</td>"
                            f"<td class='muted'>{esc(l.source.detail)}</td></tr>" for l in c.lines)
            if c.missing:
                verdict = (f"<p class='amber'>Needs {esc(', '.join(c.missing))}, which nobody sells on the market and "
                           f"no recipe makes (an NPC shop, drop or quest item). The cost below leaves it out, so this "
                           f"is only worth it if you already have it or get it cheaply.</p>")
            elif not c.priced:
                verdict = (f"<p class='amber'>No market price for it yet, so no profit estimate: cost "
                           f"{kinah(c.cost)} per attempt (success {c.success:.1%}).</p>")
            elif c.profit > 0:
                verdict = (f"<p class='amber'>Profitable on paper ({kinah(c.profit, True)} per attempt), but not "
                           f"worth it once the usual gap between predicted and real returns is taken off, or once "
                           f"Splendent pieces are limited to what their market takes per day, so it isn't listed "
                           f"in Best ways to earn.</p>")
            else:
                verdict = (f"<p class='red'>Not profitable right now: cost {kinah(c.cost)} per attempt, "
                           f"expected back {kinah(c.ev)} (success {c.success:.1%}).</p>")
            self.craft_detail.setHtml(
                f"<h2>{esc(c.name)}</h2>{verdict}"
                f"<table cellspacing=0><tr><th>INGREDIENT</th><th align=right>QTY</th><th align=right>UNIT</th>"
                f"<th>CHEAPEST SOURCE</th></tr>{lines}</table>" + self.craft_extra_html(c))

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

    # ── page 4: market ───────────────────────────────────────────────────

    def _build_market_tab(self):
        w = QWidget()
        lay = QVBoxLayout(w)
        lay.setContentsMargins(0, 4, 0, 0)
        lay.setSpacing(12)
        top = QHBoxLayout()
        top.setSpacing(8)
        self.market_section = QComboBox()
        self.market_section.addItem("All sections", None)
        self.market_section.currentIndexChanged.connect(self._apply_market_filter)
        top.addWidget(self.market_section)
        self.market_reliable = QCheckBox("Hide unreliable")
        self.market_reliable.setObjectName("Chip")
        self.market_reliable.setToolTip("Hide items with very few listings or that are not selling")
        self.market_reliable.toggled.connect(self._apply_market_filter)
        top.addWidget(self.market_reliable)
        top.addStretch(1)
        self.market_search = QLineEdit()
        self.market_search.setPlaceholderText("Search item…")
        self.market_search.setClearButtonEnabled(True)
        self.market_search.setFixedWidth(260)
        self.market_search.textChanged.connect(self._apply_market_filter)
        top.addWidget(self.market_search)
        lay.addLayout(top)
        cols = [
            Column("Item", lambda s: s.name.lower(), lambda s: s.name, width=270, bold=True),
            Column("Category", lambda s: s.category or "", lambda s: s.category or "", width=160,
                   color=lambda s: theme.MUTED),
            Column("Cheapest now", lambda s: s.ask, lambda s: kinah(s.ask), align="right", width=120),
            Column("Listings", lambda s: s.listings, align="right", width=85),
            Column("Usual price", lambda s: s.ref, lambda s: kinah(s.ref), align="right", width=115,
                   color=lambda s: theme.GOLD if s.ref else None, tip=lambda s: s.ref_basis),
            Column("24h", lambda s: s.change_pct, lambda s: "—" if s.change_pct is None else f"{s.change_pct:+.0f}%",
                   align="right", width=75,
                   color=lambda s: None if not s.change_pct else (theme.GREEN if s.change_pct > 0 else theme.RED)),
            Column("7d", lambda s: s.trend_7d, lambda s: pct(s.trend_7d, True), align="right", width=70,
                   color=lambda s: None if not s.trend_7d else (theme.GREEN if s.trend_7d > 0 else theme.RED)),
            Column("Sold / day", lambda s: s.sales_per_day, lambda s: "?" if s.sales_per_day is None else f"{s.sales_per_day:.1f}",
                   align="right", tip=lambda s: s.sales_basis, width=95),
            Column("Make it yourself", lambda s: self._source_text(s.key), lambda s: self._source_text(s.key), width=340,
                   color=lambda s: theme.GOLD, tip=lambda s: "Cheaper than buying it (e.g. Substance Morph from Odyle)"),
            Column("Notes", lambda s: len(s.flags), lambda s: ", ".join(s.flags), color=lambda s: theme.AMBER),
        ]
        self.market_model, self.market_proxy, self.market_view = make_table(cols)
        self.market_view.sortByColumn(3, Qt.DescendingOrder)
        self.market_view.selectionModel().selectionChanged.connect(self._market_selected)
        bottom = QWidget()
        bl = QHBoxLayout(bottom)
        bl.setContentsMargins(0, 0, 0, 0)
        bl.setSpacing(12)
        self.chart = PriceChart()
        bl.addWidget(self.chart, 3)
        self.market_detail = browser()
        self.market_detail.setHtml(self._empty_detail("Select an item."))
        bl.addWidget(self.market_detail, 2)
        split = QSplitter(Qt.Vertical)
        split.addWidget(self.market_view)
        split.addWidget(bottom)
        split.setSizes([450, 240])
        split.setChildrenCollapsible(False)
        lay.addWidget(split, 1)
        return w

    def _source_text(self, key):
        if not self.analysis:
            return ""
        ctx = self.analysis.ctx
        src = ctx.cheapest_source(key)
        st = self.analysis.stats.get(key)
        if not st:
            return ""
        if src and src.how not in ("buy", "bulk"):
            return f"{src.detail}: {kinah(src.cost)}" + (f" (market {kinah(st.ask)})" if st.ask else "")
        made = ctx.made.get(key)
        if made:  # buying is cheaper, but show what making it yourself costs
            return f"make it yourself: {kinah(made.cost)} ({made.detail})"
        return ""

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
        self.chart.set_data(closes[-168:], s.ref, f"{s.name} · price history")
        h = history_stats(d.get("hourly") or [])
        src = self._source_text(s.key)
        sold = "?" if s.sales_per_day is None else f"{s.sales_per_day:.1f}"
        self.market_detail.setHtml(
            f"<h2>{esc(s.name)}</h2><p class='muted'>{esc(s.category or '')}</p>"
            "<table cellspacing=0><tr>"
            + stat_cell("Cheapest now", kinah(s.ask)) + stat_cell("Usual", kinah(s.ref), theme.GOLD)
            + "</tr></table>"
            f"<p>{s.listings} listings · low end {kinah(s.pessimistic)} · swings ±{pct(s.volatility)}<br>"
            f"Sold per day: <b>{sold}</b> <span class='muted'>({esc(s.sales_basis)})</span><br>"
            + (f"Real sales (28 d): {s.units_sold_28d:,} at avg {kinah(s.sold_avg)}<br>" if s.units_sold_28d else "")
            + (f"Bulk buy now: {kinah(s.bulk_unit)} avg for {s.bulk_units} units<br>" if s.bulk_unit else "")
            + (f"<span class='gold'>Cheaper: {esc(src)}</span><br>" if src else "")
            + (f"<span class='amber'>⚠ {esc(', '.join(s.flags))}</span><br>" if s.flags else "")
            + f"<span class='muted'>Cheapest now · {h['n']} hourly points in the last 72 h · "
              f"usual price: {esc(s.ref_basis)}</span></p>")

    # ── page 5: supply requests ──────────────────────────────────────────

    def _build_supply_tab(self):
        w = QWidget()
        lay = QVBoxLayout(w)
        lay.setContentsMargins(0, 4, 0, 0)
        lay.setSpacing(12)
        intro = muted("Supply requests ask players to hand in items: daily ones (Emergency) and weekly ones. While a "
                      "request runs, more people buy those items, so they sell faster and hold their price; when it "
                      "ends, those buyers are gone. There is no public list of active requests and the app never reads "
                      "the game, so add what your request board shows. Items count for a day (daily) or a week "
                      "(weekly) unless you set the hours left.")
        intro.setWordWrap(True)
        lay.addWidget(intro)
        row = QHBoxLayout()
        row.setSpacing(8)
        self.supply_item = QLineEdit()
        self.supply_item.setPlaceholderText("Item name, e.g. Radiant Odyle")
        self.supply_names = QStringListModel()
        self.supply_completer = QCompleter(self.supply_names, self)
        self.supply_completer.setCaseSensitivity(Qt.CaseInsensitive)
        self.supply_completer.setFilterMode(Qt.MatchContains)
        self.supply_item.setCompleter(self.supply_completer)
        row.addWidget(self.supply_item, 1)
        row.addWidget(muted("Needed"))
        self.supply_qty = QSpinBox()
        self.supply_qty.setRange(1, 9999)
        self.supply_qty.setFixedWidth(90)
        row.addWidget(self.supply_qty)
        self.supply_kind = QComboBox()
        self.supply_kind.addItems(list(supply_mod.KINDS))
        self.supply_kind.setCurrentText("Weekly")
        row.addWidget(self.supply_kind)
        row.addWidget(muted("Hours left"))
        self.supply_hours = QSpinBox()
        self.supply_hours.setRange(0, 24 * 7)
        self.supply_hours.setSpecialValueText("auto")
        self.supply_hours.setFixedWidth(90)
        self.supply_hours.setToolTip("0 = a full day (daily) or a full week (weekly) from now")
        row.addWidget(self.supply_hours)
        self.supply_add = QPushButton("Add request")
        self.supply_add.setObjectName("Primary")
        self.supply_add.setCursor(Qt.PointingHandCursor)
        self.supply_add.clicked.connect(self._supply_add)
        self.supply_item.returnPressed.connect(self._supply_add)
        row.addWidget(self.supply_add)
        lay.addLayout(row)
        self.supply_msg = muted("")
        lay.addWidget(self.supply_msg)
        cols = [
            Column("Type", lambda r: r.kind, color=lambda r: theme.VIOLET if r.kind == "Weekly" else theme.BLUE,
                   pill=True, width=110),
            Column("Item", lambda r: r.name.lower(), lambda r: r.name, width=300, bold=True),
            Column("Needed", lambda r: r.qty, align="right", width=90),
            Column("Time left", lambda r: r.hours_left(), lambda r: self._left_text(r), align="right", width=110,
                   color=lambda r: theme.RED if not r.active() else (theme.AMBER if r.hours_left() <= 24 else None)),
            Column("Cheapest now", lambda r: self._st(r.key, "ask"), lambda r: kinah(self._st(r.key, "ask")),
                   align="right", width=120),
            Column("Usual price", lambda r: self._st(r.key, "ref"), lambda r: kinah(self._st(r.key, "ref")),
                   align="right", width=115, color=lambda r: theme.GOLD),
            Column("Sold / day", lambda r: self._st(r.key, "sales_per_day"),
                   lambda r: "?" if self._st(r.key, "sales_per_day") is None else f"{self._st(r.key, 'sales_per_day'):.1f}",
                   align="right", tip=lambda r: f"Includes about +{supply_mod.DEMAND_BOOST - 1:.0%} extra buyers "
                                                f"while the request runs"),
        ]
        self.supply_model, self.supply_proxy, self.supply_view = make_table(cols)
        lay.addWidget(self.supply_view, 1)
        bottom = QHBoxLayout()
        self.supply_remove = QPushButton("Remove selected")
        self.supply_remove.clicked.connect(self._supply_remove)
        bottom.addWidget(self.supply_remove)
        self.supply_clear = QPushButton("Remove expired")
        self.supply_clear.clicked.connect(self._supply_clear_expired)
        bottom.addWidget(self.supply_clear)
        bottom.addStretch(1)
        lay.addLayout(bottom)
        self._supply_refresh_table()
        return w

    def _st(self, key, attr):
        st = self.analysis.stats.get(key) if self.analysis else None
        return getattr(st, attr) if st else None

    @staticmethod
    def _left_text(r):
        h = r.hours_left()
        if h <= 0:
            return "expired"
        return f"{h:.0f} h" if h < 48 else f"{h / 24:.1f} days"

    def _item_names(self):
        names = {}
        if self.analysis:
            for k, st in self.analysis.stats.items():
                names[st.name.lower()] = (k, st.name)
        return names

    def _supply_add(self):
        text = self.supply_item.text().strip()
        if not text:
            return
        names = self._item_names()
        hit = names.get(text.lower())
        if not hit:
            matches = [v for n, v in names.items() if text.lower() in n]
            hit = matches[0] if len(matches) == 1 else None
        if not hit:
            self.supply_msg.setText("Item not found on your server's market"
                                    + ("" if self.analysis else " — press Refresh first") + ". Pick a name from the list.")
            self.supply_msg.setStyleSheet(f"color:{theme.RED}")
            return
        key, name = hit
        hours = self.supply_hours.value() or None
        req = supply_mod.new_request(key, name, self.supply_kind.currentText(), self.supply_qty.value(),
                                     hours_left=hours)
        self.settings.supply_requests = [r for r in self.settings.supply_requests if r.key != key] + [req]
        self.supply_msg.setText(f"Added {name}: counted as extra demand for {self._left_text(req)}.")
        self.supply_msg.setStyleSheet(f"color:{theme.GREEN}")
        self.supply_item.clear()
        self._supply_changed()

    def _supply_remove(self):
        r = selected_row(self.supply_view)
        if r:
            self.settings.supply_requests = [x for x in self.settings.supply_requests if x is not r]
            self._supply_changed()

    def _supply_clear_expired(self):
        self.settings.supply_requests = [x for x in self.settings.supply_requests if x.active()]
        self._supply_changed()

    def _supply_changed(self):
        self._save_settings()
        self._supply_refresh_table()
        self.schedule_recalc()

    def _supply_refresh_table(self):
        self.supply_model.set_rows(self.settings.supply_requests)
        if self.analysis:
            self.supply_names.setStringList(sorted({st.name for st in self.analysis.stats.values()}))

    # ── page 6: settings ─────────────────────────────────────────────────

    def _build_settings_tab(self):
        w = QWidget()
        outer = QVBoxLayout(w)
        outer.setContentsMargins(0, 4, 0, 0)
        outer.setSpacing(12)
        grid = QGridLayout()
        grid.setSpacing(12)

        lv = QVBoxLayout()
        t = QLabel("CRAFTING LEVELS")
        t.setObjectName("SectionTitle")
        lv.addWidget(t)
        lv.addWidget(muted("Your proficiency in each profession. 0 = you don't have it."))
        form = QFormLayout()
        form.setSpacing(10)
        form.setLabelAlignment(Qt.AlignLeft)
        self.level_spins = {}
        for k, label in PROFESSIONS.items():
            sp = QSpinBox()
            sp.setRange(0, 200)
            sp.setFixedWidth(110)
            sp.setValue(int(self.settings.levels.get(k, 0)))
            sp.valueChanged.connect(self._levels_changed)
            form.addRow(label, sp)
            self.level_spins[k] = sp
        lv.addSpacing(6)
        lv.addLayout(form)
        grid.addWidget(card(lv), 0, 0)

        tx = QVBoxLayout()
        t2 = QLabel("SERVER, MARKET & TIME")
        t2.setObjectName("SectionTitle")
        tx.addWidget(t2)
        tx.addWidget(muted("10% tax + 2% fee per most guides. Max 10 listings at once, ~5 min each. "
                           "Server and region pick which market is read."))
        tf = QFormLayout()
        tf.setSpacing(10)
        self.tax_spin = QDoubleSpinBox()
        self.tax_spin.setRange(0, 50)
        self.tax_spin.setSuffix(" %")
        self.tax_spin.setFixedWidth(110)
        self.tax_spin.setValue(self.settings.tax_pct)
        self.tax_spin.valueChanged.connect(self._fees_changed)
        self.fee_spin = QDoubleSpinBox()
        self.fee_spin.setRange(0, 20)
        self.fee_spin.setSuffix(" %")
        self.fee_spin.setFixedWidth(110)
        self.fee_spin.setValue(self.settings.fee_pct)
        self.fee_spin.valueChanged.connect(self._fees_changed)
        tf.addRow("Sales tax (when it sells)", self.tax_spin)
        tf.addRow("Listing fee (when you list)", self.fee_spin)
        self.hours_spin = QDoubleSpinBox()
        self.hours_spin.setRange(0.5, 24)
        self.hours_spin.setDecimals(1)
        self.hours_spin.setSuffix(" h")
        self.hours_spin.setFixedWidth(110)
        self.hours_spin.setValue(self.settings.play_hours)
        self.hours_spin.setToolTip("Each listing takes ~5 min and only 10 can be up at once, so your time limits sales")
        self.hours_spin.valueChanged.connect(self._hours_changed)
        tf.addRow("Hours you play per day", self.hours_spin)
        self.share_spin = QSpinBox()
        self.share_spin.setRange(5, 100)
        self.share_spin.setSuffix(" %")
        self.share_spin.setFixedWidth(110)
        self.share_spin.setValue(int(round(self.settings.max_share * 100)))
        self.share_spin.setToolTip("Most of your budget the plan puts into one item. 35% earned the most in 501,120 "
                                   "simulated runs; up to 50% can earn a bit more but has worse bad-luck results.")
        self.share_spin.valueChanged.connect(self._share_changed)
        tf.addRow("Most budget in one item", self.share_spin)
        from ..engine import REGIONS
        self.server_edit = QLineEdit(self.settings.server_name)
        self.server_edit.setFixedWidth(140)
        self.server_edit.editingFinished.connect(self._server_changed)
        self.region_box = QComboBox()
        for code, label in REGIONS.items():
            self.region_box.addItem(label, code)
        self.region_box.setCurrentIndex(max(0, self.region_box.findData(self.settings.server_region)))
        self.region_box.currentIndexChanged.connect(self._server_changed)
        tf.addRow("Server", self.server_edit)
        tf.addRow("Region", self.region_box)
        tx.addSpacing(6)
        tx.addLayout(tf)
        tx.addStretch(1)
        grid.addWidget(card(tx), 0, 1)
        outer.addLayout(grid)

        info = browser()
        info.setHtml(
            "<h2>How it works</h2>"
            "<p>Prices come from the public <a href='https://aion2.exchange'>aion2.exchange</a> website, for the server and region set above. "
            "The app only reads that website — it never touches the game, its files or its memory.</p>"
            "<p><span class='gold'>Your own history.</span> Every Refresh is saved, so the app builds its own price "
            "and sales history. Refresh a few times a day and the sales-speed estimates keep getting better.</p>"
            "<p><span class='gold'>Usual price</span> is the median of the last 72 hours, so one odd listing can't "
            "fool it. Items with fewer than 3 listings, or that sit on the market without selling, are never used "
            "to claim a profit.</p>"
            "<p><span class='gold'>Odyle → materials.</span> Every Odyle grade morphs 1:1 into ore, gems, logs and "
            "plants of the same grade. Morphed materials can't be sold, so the app uses this only when Odyle is "
            "cheaper.</p>"
            "<p><span class='gold'>Also counted:</span> crafting intermediates yourself when cheaper, alternative "
            "ingredients, NPC supplies, the Splendent combo chance, failed crafts eating materials, and bulk prices "
            "when the cheapest listing runs out.</p>"
            "<p><span class='gold'>Profit per day</span> is the main ranking: kinah that comes back in hours can be "
            "spent again, so a small margin that sells fast can beat a big one that sits for a week.</p>"
            f"<p class='muted'>Data folder: {esc(self.engine.data_dir)} · Recipes: gamers4.life crafting database</p>")
        outer.addWidget(info, 1)
        return w

    def _levels_changed(self):
        self.settings.levels = {k: sp.value() for k, sp in self.level_spins.items()}
        self._save_settings()
        self._update_banner()
        self.schedule_recalc()

    def _server_changed(self):
        from ..engine import REGIONS
        name = self.server_edit.text().strip() or "Lumiel"
        region = self.region_box.currentData()
        if (name, region) == (self.settings.server_name, self.settings.server_region):
            return
        self.settings.server_name, self.settings.server_region = name, region
        self._save_settings()
        self.server_label.setText(name.upper())
        self.server_sub.setText(REGIONS.get(region, region))
        self.analysis = None
        self.refresh()  # a different market: download it

    def _hours_changed(self):
        self.settings.play_hours = self.hours_spin.value()
        self._save_settings()
        self._update_plan()

    def _share_changed(self):
        self.settings.max_share = self.share_spin.value() / 100.0
        self._save_settings()
        self._update_plan()

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
            msgs.append("Set your crafting levels in Settings to see crafting opportunities.")
        if self.analysis:
            msgs.extend(self.analysis.data.warnings)
            if self.analysis.data.snapshot_count < 3:
                msgs.append("Few refreshes saved yet: sales-speed estimates get better each time you press Refresh "
                            "(a few times a day is ideal).")
        self.banner.setText("ⓘ  " + "\nⓘ  ".join(msgs) if msgs else "")
        self.banner.setVisible(bool(msgs))

    def _update_kpis(self):
        a = self.analysis
        if not a:
            for k in (self.kpi_best, self.kpi_safe, self.kpi_fast, self.kpi_items):
                k.set("—", "press Refresh")
            return
        ops = a.ops
        shown = [o for o in ops if o.risk in self.settings.risks]
        best = max(shown, key=lambda o: o.real_profit_day, default=None)
        self.kpi_best.set("+" + short(best.real_profit_day) if best else "—",
                          best.name if best else "nothing in your risk levels")
        safe = [o for o in ops if o.risk == "Safe"]
        self.kpi_safe.set(str(len(safe)), f"of {len(ops)} opportunities")
        fast = [o for o in ops if o.fast]
        self.kpi_fast.set(str(len(fast)), "sell within a day")
        self.kpi_items.set(f"{len(a.stats):,}", f"{a.data.snapshot_count} refresh{'' if a.data.snapshot_count == 1 else 'es'} saved")

    # ── refresh / recalc ─────────────────────────────────────────────────

    def refresh(self):
        if self._thread is not None:
            self._refresh_again = True  # e.g. the server changed mid-refresh: run once more after this one
            return
        self.refresh_btn.setEnabled(False)
        self.refresh_btn.setText("  Refreshing…")
        self.progress.setValue(0)
        self.progress.show()
        self._set_status("Refreshing…", theme.VIOLET)
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
        self._set_status(msg, theme.VIOLET)
        self.progress.setValue(int(frac * 100))

    @Slot(object)
    def _on_refreshed(self, analysis):
        used = self._worker.settings if self._worker else self.settings
        if (used.server_name, used.server_region) != (self.settings.server_name, self.settings.server_region):
            return  # downloaded for the server you just switched away from; the next refresh is queued
        self.analysis = analysis
        srv = analysis.data.server
        self.server_label.setText(str(srv.get("name", self.settings.server_name)).upper())
        self.server_sub.setText(f"{srv.get('faction', '')} · {srv.get('region_name') or srv.get('region', '')}".strip(" ·"))
        self.recalculate(reuse=used == self.settings)  # settings edited mid-refresh: recalculate with them
        when = time.strftime("%H:%M", time.localtime(analysis.data.fetched_at))
        if analysis.data.offline:
            self._set_status("Offline copy from " + when, theme.AMBER)
        else:
            self._set_status("Updated " + when, theme.GREEN)

    @Slot(str)
    def _on_failed(self, msg):
        self._set_status("Refresh failed", theme.RED)
        QMessageBox.warning(self, "Refresh failed", msg)

    @Slot()
    def _thread_done(self):
        self.progress.hide()
        self.refresh_btn.setEnabled(True)
        self.refresh_btn.setText("  Refresh")
        self._thread.deleteLater()
        self._thread = None
        self._worker = None
        if self._refresh_again:
            self._refresh_again = False
            QTimer.singleShot(0, self.refresh)

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
        self.craft_model.set_rows(a.crafts + a.splendents)
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
            f"{len(a.crafts)} recipes you can craft at your levels ({sum(1 for c in a.crafts if c.priced)} with a "
            f"market price) + {len(a.splendents)} Splendent items (✦) you get from their combo procs. Levels apply "
            f"as soon as you change them in Settings."
            if n_levels else "Set your crafting levels in Settings to see recipes.")
        self._update_plan()
        self._update_banner()
        self._update_kpis()
        self._supply_refresh_table()
        self.weekly_label.setText("WEEKLY CYCLE  ·  " + supply_mod.weekly_tip())

    def closeEvent(self, event):
        if self._thread is not None:
            self._thread.quit()
            self._thread.wait(3000)
        super().closeEvent(event)
