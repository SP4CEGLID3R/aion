"""The shopping list window: what to buy for the trades you ticked on Best ways to earn."""
from __future__ import annotations

from PySide6.QtCore import Qt
from PySide6.QtWidgets import (
    QApplication, QDialog, QHBoxLayout, QHeaderView, QLabel, QPushButton, QSpinBox, QTableWidget,
    QTableWidgetItem, QVBoxLayout,
)

from ..shopping import shopping_list
from . import theme
from .widgets import kinah


def _item(text, right=False):
    it = QTableWidgetItem(text)
    it.setFlags(Qt.ItemIsEnabled)
    if right:
        it.setTextAlignment(Qt.AlignRight | Qt.AlignVCenter)
    return it


class ShoppingDialog(QDialog):
    def __init__(self, ctx, ops, parent=None):
        super().__init__(parent)
        self.ctx = ctx
        self.ops = ops
        self.setWindowTitle("Shopping list")
        self.resize(1000, 720)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(18, 16, 18, 16)
        lay.setSpacing(10)
        head = QLabel("Shopping list")
        head.setObjectName("PageTitle")
        lay.addWidget(head)
        lay.addWidget(self._muted("Set how many crafts (or items to resell) you want. Everything you craft "
                                  "yourself on the way, Splendent bases from combo procs and Substance Morph is "
                                  "broken down into what you actually buy. Quantities are averages that include "
                                  "failed crafts, so buy a little extra to be safe."))

        self.picks = QTableWidget(len(ops), 4)
        self.picks.setHorizontalHeaderLabels(["ITEM", "METHOD", "HOW MANY", "PROFIT (REALISTIC)"])
        self.picks.verticalHeader().setVisible(False)
        self.picks.setShowGrid(False)
        self.spins = []
        for i, op in enumerate(ops):
            self.picks.setItem(i, 0, _item(op.name))
            self.picks.setItem(i, 1, _item("craft attempts" if op.method == "Craft" else "buy to resell"))
            sp = QSpinBox()
            sp.setRange(0, 100000)
            sp.setValue(max(1, op.batch))
            sp.valueChanged.connect(self.recalc)
            self.spins.append(sp)
            self.picks.setCellWidget(i, 2, sp)
            self.picks.setItem(i, 3, _item("", right=True))
        self._fit(self.picks, stretch=0)
        self.picks.setMaximumHeight(46 + 40 * min(len(ops), 5))
        lay.addWidget(self.picks)

        self.mats = QTableWidget(0, 5)
        self.mats.setHorizontalHeaderLabels(["BUY", "MATERIAL", "UNIT PRICE", "TOTAL", "WHERE"])
        self.mats.verticalHeader().setVisible(False)
        self.mats.setShowGrid(False)
        self._fit(self.mats, stretch=4)
        lay.addWidget(self.mats, 1)

        self.summary = QLabel("")
        self.summary.setWordWrap(True)
        self.summary.setStyleSheet(f"font-size:11pt;color:{theme.TEXT}")
        lay.addWidget(self.summary)
        row = QHBoxLayout()
        row.addStretch(1)
        copy_btn = QPushButton("Copy as text")
        copy_btn.clicked.connect(self.copy)
        close = QPushButton("Close")
        close.setObjectName("Primary")
        close.clicked.connect(self.accept)
        row.addWidget(copy_btn)
        row.addWidget(close)
        lay.addLayout(row)
        self.current = None
        self.recalc()

    @staticmethod
    def _muted(text):
        lab = QLabel(text)
        lab.setWordWrap(True)
        lab.setStyleSheet(f"color:{theme.TEXT_2}")
        return lab

    @staticmethod
    def _fit(table, stretch):
        h = table.horizontalHeader()
        h.setSectionResizeMode(QHeaderView.ResizeToContents)
        h.setSectionResizeMode(stretch, QHeaderView.Stretch)
        table.verticalHeader().setDefaultSectionSize(34)
        table.setAlternatingRowColors(True)
        table.setEditTriggers(QTableWidget.NoEditTriggers)

    def recalc(self):
        picks = [(op, sp.value()) for op, sp in zip(self.ops, self.spins)]
        sl = shopping_list(self.ctx, picks)
        self.current = sl
        for i, (op, n) in enumerate(picks):
            self.picks.item(i, 3).setText(kinah(op.real_profit_unit * n, True))
        rows = sl.rows()
        self.mats.setRowCount(len(rows) + (1 if sl.fees else 0))
        for i, m in enumerate(rows):
            self.mats.setItem(i, 0, _item(f"{m.buy_qty:,}", right=True))
            self.mats.setItem(i, 1, _item(m.name))
            self.mats.setItem(i, 2, _item(kinah(m.unit), right=True))
            self.mats.setItem(i, 3, _item(kinah(m.buy_qty * m.unit), right=True))
            self.mats.setItem(i, 4, _item(m.where))
        if sl.fees:
            i = len(rows)
            self.mats.setItem(i, 0, _item(""))
            self.mats.setItem(i, 1, _item("Crafting fees"))
            self.mats.setItem(i, 2, _item(""))
            self.mats.setItem(i, 3, _item(kinah(sl.fees), right=True))
            self.mats.setItem(i, 4, _item("kinah, paid when crafting"))
        profit = sum(op.real_profit_unit * n for op, n in picks)
        extra = ""
        if sl.byproducts:
            extra = ("<br><span style='color:%s'>On the way you also make (to sell): %s</span>" % (
                theme.TEXT_2, ", ".join(f"~{q:,.0f} × {name}" for name, q in sl.byproducts.values())))
        crafts = ", ".join(f"~{n:,.0f} × {name}" for name, n in sl.attempts.items())
        self.summary.setText(
            f"<b>Total to spend: <span style='color:{theme.GOLD}'>{kinah(sl.total)}</span> kinah</b> · "
            f"expected profit <span style='color:{theme.GREEN}'>{kinah(profit, True)}</span> (realistic)"
            + (f"<br><span style='color:{theme.TEXT_2}'>Crafts to make: {crafts}</span>" if crafts else "") + extra)

    def copy(self):
        if self.current:
            QApplication.clipboard().setText(self.current.as_text())
