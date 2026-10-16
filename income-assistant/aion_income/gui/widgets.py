"""Small reusable pieces: number formatting, a sortable table model, a price chart."""
from __future__ import annotations

import re

from PySide6.QtCore import QAbstractTableModel, QModelIndex, QSortFilterProxyModel, Qt
from PySide6.QtGui import QBrush, QColor, QPainter, QPainterPath, QPen
from PySide6.QtWidgets import QHeaderView, QTableView, QWidget

RISK_COLORS = {"Safe": "#1b8a3a", "Moderate": "#b8860b", "Risky": "#c0392b"}


def kinah(v, signed=False) -> str:
    if v is None:
        return "—"
    s = f"{v:+,.0f}" if signed else f"{v:,.0f}"
    return s


def short(v) -> str:
    if v is None:
        return "—"
    a = abs(v)
    if a >= 1e9:
        return f"{v / 1e9:.2f}B"
    if a >= 1e6:
        return f"{v / 1e6:.2f}M"
    if a >= 1e4:
        return f"{v / 1e3:.0f}K"
    return f"{v:,.0f}"


def pct(v, signed=False) -> str:
    if v is None:
        return "—"
    return f"{v:+.0%}" if signed else f"{v:.0%}"


def parse_kinah(text: str) -> float | None:
    """'5m', '5.5kk', '500k', '5 000 000', '2b' -> kinah."""
    t = text.strip().lower().replace(" ", "").replace(",", "").replace("_", "")
    if not t:
        return None
    m = re.fullmatch(r"(\d+(?:\.\d+)?)(kkk|kk|k|m|mil|b)?", t)
    if not m:
        return None
    mult = {None: 1, "k": 1e3, "kk": 1e6, "m": 1e6, "mil": 1e6, "kkk": 1e9, "b": 1e9}[m.group(2)]
    return float(m.group(1)) * mult


class Column:
    def __init__(self, title, value, text=None, tip=None, color=None, align=None, width=None):
        self.title = title
        self.width = width
        self.value = value          # row -> sortable value
        self.text = text or (lambda r: "" if value(r) is None else str(value(r)))
        self.tip = tip
        self.color = color
        self.align = align


class RowsModel(QAbstractTableModel):
    def __init__(self, columns, parent=None):
        super().__init__(parent)
        self.columns = columns
        self.rows = []

    def set_rows(self, rows):
        self.beginResetModel()
        self.rows = list(rows)
        self.endResetModel()

    def rowCount(self, parent=QModelIndex()):
        return 0 if parent.isValid() else len(self.rows)

    def columnCount(self, parent=QModelIndex()):
        return 0 if parent.isValid() else len(self.columns)

    def headerData(self, section, orientation, role=Qt.DisplayRole):
        if orientation == Qt.Horizontal and role == Qt.DisplayRole:
            return self.columns[section].title
        return None

    def data(self, index, role=Qt.DisplayRole):
        if not index.isValid():
            return None
        row = self.rows[index.row()]
        col = self.columns[index.column()]
        if role == Qt.DisplayRole:
            return col.text(row)
        if role == Qt.UserRole:
            v = col.value(row)
            return v
        if role == Qt.UserRole + 1:
            return row
        if role == Qt.ToolTipRole and col.tip:
            return col.tip(row)
        if role == Qt.ForegroundRole and col.color:
            c = col.color(row)
            return QBrush(QColor(c)) if c else None
        if role == Qt.TextAlignmentRole:
            if col.align == "right":
                return int(Qt.AlignRight | Qt.AlignVCenter)
        return None


class RowsProxy(QSortFilterProxyModel):
    def __init__(self, parent=None):
        super().__init__(parent)
        self.predicate = lambda row: True
        self.setSortRole(Qt.UserRole)

    def set_predicate(self, fn):
        self.predicate = fn
        self.invalidateFilter()

    def filterAcceptsRow(self, source_row, source_parent):
        model = self.sourceModel()
        return bool(self.predicate(model.rows[source_row]))

    def lessThan(self, left, right):
        a = left.data(Qt.UserRole)
        b = right.data(Qt.UserRole)
        # unknown values always sort last-ish (treated as smallest)
        if a is None and b is None:
            return False
        if a is None:
            return True
        if b is None:
            return False
        try:
            return a < b
        except TypeError:
            return str(a) < str(b)


def make_table(columns, parent=None):
    model = RowsModel(columns, parent)
    proxy = RowsProxy(parent)
    proxy.setSourceModel(model)
    view = QTableView(parent)
    view.setModel(proxy)
    view.setSortingEnabled(True)
    view.setSelectionBehavior(QTableView.SelectRows)
    view.setSelectionMode(QTableView.SingleSelection)
    view.setAlternatingRowColors(True)
    view.verticalHeader().setVisible(False)
    view.setEditTriggers(QTableView.NoEditTriggers)
    view.setWordWrap(False)
    h = view.horizontalHeader()
    h.setSectionResizeMode(QHeaderView.Interactive)
    h.setStretchLastSection(True)
    for i, c in enumerate(columns):
        view.setColumnWidth(i, c.width or (90 if c.align == "right" else 110))
    return model, proxy, view


def selected_row(view: QTableView):
    idx = view.selectionModel().selectedRows() if view.selectionModel() else []
    if not idx:
        return None
    return idx[0].data(Qt.UserRole + 1)


class PriceChart(QWidget):
    """Hourly cheapest-listing line with the typical price as a dashed line."""

    def __init__(self, parent=None):
        super().__init__(parent)
        self.points = []
        self.ref = None
        self.setMinimumHeight(140)

    def set_data(self, points, ref=None):
        self.points = [p for p in points if p is not None]
        self.ref = ref
        self.update()

    def paintEvent(self, event):
        p = QPainter(self)
        p.setRenderHint(QPainter.Antialiasing)
        r = self.rect().adjusted(50, 10, -10, -20)
        pal = self.palette()
        p.setPen(pal.color(pal.ColorRole.Mid))
        p.drawRect(r)
        if len(self.points) < 2:
            p.setPen(pal.color(pal.ColorRole.Text))
            p.drawText(self.rect(), Qt.AlignCenter, "No price history yet")
            return
        vals = self.points + ([self.ref] if self.ref else [])
        lo, hi = min(vals), max(vals)
        if hi - lo < 1e-9:
            hi = lo + 1
        def xy(i, v):
            x = r.left() + r.width() * i / (len(self.points) - 1)
            y = r.bottom() - r.height() * (v - lo) / (hi - lo)
            return x, y
        path = QPainterPath()
        for i, v in enumerate(self.points):
            x, y = xy(i, v)
            (path.moveTo if i == 0 else path.lineTo)(x, y)
        p.setPen(QPen(QColor("#2e86de"), 2))
        p.drawPath(path)
        if self.ref:
            _, y = xy(0, self.ref)
            p.setPen(QPen(QColor("#888888"), 1, Qt.DashLine))
            p.drawLine(r.left(), int(y), r.right(), int(y))
        p.setPen(pal.color(pal.ColorRole.Text))
        p.drawText(2, r.top() + 10, short(hi))
        p.drawText(2, r.bottom(), short(lo))
        p.drawText(r.left(), self.height() - 4, "older")
        p.drawText(r.right() - 40, self.height() - 4, "now")
