"""Reusable pieces: number formatting, sortable tables, risk pills, KPI cards, the price chart."""
from __future__ import annotations

import re

from PySide6.QtCore import QAbstractTableModel, QModelIndex, QRectF, QSortFilterProxyModel, Qt, Signal
from PySide6.QtGui import QBrush, QColor, QLinearGradient, QPainter, QPainterPath, QPen
from PySide6.QtWidgets import (
    QFrame, QHeaderView, QLabel, QStyle, QStyledItemDelegate, QTableView, QVBoxLayout, QWidget,
)

from . import theme


def kinah(v, signed=False) -> str:
    if v is None:
        return "—"
    return f"{v:+,.0f}" if signed else f"{v:,.0f}"


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
    def __init__(self, title, value, text=None, tip=None, color=None, align=None, width=None, pill=False,
                 bold=False, check=None):
        self.title = title
        self.value = value          # row -> sortable value
        self.text = text or (lambda r: "" if value(r) is None else str(value(r)))
        self.tip = tip
        self.color = color          # row -> colour string or None
        self.align = align
        self.width = width
        self.pill = pill            # draw as a coloured pill (risk levels, methods)
        self.bold = bold
        self.check = check          # row -> key: a tick box in this column (ticks survive refreshes)


class RowsModel(QAbstractTableModel):
    checked_changed = Signal()

    def __init__(self, columns, parent=None):
        super().__init__(parent)
        self.columns = columns
        self.rows = []
        self.checked: set = set()   # keys of ticked rows

    def checked_rows(self) -> list:
        col = next((c for c in self.columns if c.check), None)
        return [r for r in self.rows if col and col.check(r) in self.checked]

    def flags(self, index):
        f = super().flags(index)
        if index.isValid() and self.columns[index.column()].check:
            f |= Qt.ItemIsUserCheckable
        return f

    def setData(self, index, value, role=Qt.EditRole):
        col = self.columns[index.column()] if index.isValid() else None
        if role != Qt.CheckStateRole or not col or not col.check:
            return False
        key = col.check(self.rows[index.row()])
        on = value in (Qt.Checked, Qt.Checked.value, 2)
        (self.checked.add if on else self.checked.discard)(key)
        self.dataChanged.emit(index, index, [Qt.CheckStateRole])
        self.checked_changed.emit()
        return True

    def set_rows(self, rows):
        self.beginResetModel()
        self.rows = list(rows)
        self.endResetModel()

    def rowCount(self, parent=QModelIndex()):
        return 0 if parent.isValid() else len(self.rows)

    def columnCount(self, parent=QModelIndex()):
        return 0 if parent.isValid() else len(self.columns)

    def headerData(self, section, orientation, role=Qt.DisplayRole):
        if orientation == Qt.Horizontal:
            if role == Qt.DisplayRole:
                return self.columns[section].title.upper()
            if role == Qt.TextAlignmentRole:
                a = Qt.AlignRight if self.columns[section].align == "right" else Qt.AlignLeft
                return int(a | Qt.AlignVCenter)
        return None

    def data(self, index, role=Qt.DisplayRole):
        if not index.isValid():
            return None
        row = self.rows[index.row()]
        col = self.columns[index.column()]
        if role == Qt.CheckStateRole and col.check:
            return Qt.Checked if col.check(row) in self.checked else Qt.Unchecked
        if role == Qt.DisplayRole:
            return col.text(row)
        if role == Qt.UserRole:
            return col.value(row)
        if role == Qt.UserRole + 1:
            return row
        if role == Qt.UserRole + 2:
            return col.color(row) if col.color else None
        if role == Qt.ToolTipRole and col.tip:
            return col.tip(row)
        if role == Qt.ForegroundRole and col.color and not col.pill:
            c = col.color(row)
            return QBrush(QColor(c)) if c else None
        if role == Qt.FontRole and col.bold:
            return theme.app_font(10, 600)
        if role == Qt.TextAlignmentRole:
            if col.align == "right":
                return int(Qt.AlignRight | Qt.AlignVCenter)
            return int(Qt.AlignLeft | Qt.AlignVCenter)
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


class PillDelegate(QStyledItemDelegate):
    """Draws the cell text inside a rounded, tinted pill of the cell's colour."""

    def paint(self, painter, option, index):
        self.initStyleOption(option, index)
        text = option.text
        option.text = ""
        option.widget.style().drawControl(QStyle.CE_ItemViewItem, option, painter, option.widget)
        color = QColor(index.data(Qt.UserRole + 2) or theme.MUTED)
        painter.save()
        painter.setRenderHint(QPainter.Antialiasing)
        fm = option.fontMetrics
        w = fm.horizontalAdvance(text) + 22
        h = 22
        r = QRectF(option.rect.left() + 8, option.rect.center().y() - h / 2 + 1, w, h)
        fill = QColor(color)
        fill.setAlpha(38)
        painter.setPen(Qt.NoPen)
        painter.setBrush(fill)
        painter.drawRoundedRect(r, h / 2, h / 2)
        painter.setBrush(color)
        painter.drawEllipse(r.left() + 9, r.center().y() - 2.5, 5, 5)
        painter.setPen(color)
        f = theme.app_font(9, 600)
        painter.setFont(f)
        painter.drawText(r.adjusted(14, 0, 0, 0), Qt.AlignCenter, text)
        painter.restore()


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
    view.setShowGrid(False)
    view.setMouseTracking(True)
    view.verticalHeader().setVisible(False)
    view.verticalHeader().setDefaultSectionSize(38)
    view.setEditTriggers(QTableView.NoEditTriggers)
    view.setWordWrap(False)
    view.setFocusPolicy(Qt.NoFocus)
    view.setHorizontalScrollMode(QTableView.ScrollPerPixel)
    view.setVerticalScrollMode(QTableView.ScrollPerPixel)
    h = view.horizontalHeader()
    h.setSectionResizeMode(QHeaderView.Interactive)
    h.setStretchLastSection(True)
    h.setHighlightSections(False)
    delegate = PillDelegate(view)
    view._pill_delegate = delegate  # keep a reference
    for i, c in enumerate(columns):
        view.setColumnWidth(i, c.width or (110 if c.align == "right" else 120))
        if c.pill:
            view.setItemDelegateForColumn(i, delegate)
    return model, proxy, view


def selected_row(view: QTableView):
    idx = view.selectionModel().selectedRows() if view.selectionModel() else []
    if not idx:
        return None
    return idx[0].data(Qt.UserRole + 1)


class KpiCard(QFrame):
    """A headline number: small caps title, big value, faint hint."""

    def __init__(self, title: str, accent: str = theme.TEXT, parent=None):
        super().__init__(parent)
        self.setObjectName("KpiCard")
        self.accent = accent
        lay = QVBoxLayout(self)
        lay.setContentsMargins(16, 12, 16, 12)
        lay.setSpacing(2)
        self.title = QLabel(title.upper())
        self.title.setObjectName("KpiTitle")
        self.value = QLabel("—")
        self.value.setObjectName("KpiValue")
        self.value.setStyleSheet(f"color:{accent}")
        self.hint = QLabel("")
        self.hint.setObjectName("KpiHint")
        lay.addWidget(self.title)
        lay.addWidget(self.value)
        lay.addWidget(self.hint)

    def set(self, value: str, hint: str = "", color: str | None = None):
        self.value.setText(value)
        self.hint.setText(hint)
        self.value.setStyleSheet(f"color:{color or self.accent}")


class PriceChart(QWidget):
    """Hourly cheapest-listing line over a soft gradient, with the usual price dashed."""

    def __init__(self, parent=None):
        super().__init__(parent)
        self.points = []
        self.ref = None
        self.title = ""
        self.setMinimumHeight(170)

    def set_data(self, points, ref=None, title=""):
        self.points = [p for p in points if p is not None]
        self.ref = ref
        self.title = title
        self.update()

    def paintEvent(self, event):
        p = QPainter(self)
        p.setRenderHint(QPainter.Antialiasing)
        outer = QRectF(self.rect()).adjusted(0.5, 0.5, -0.5, -0.5)
        p.setPen(QPen(QColor(theme.BORDER), 1))
        p.setBrush(QColor(theme.SURFACE))
        p.drawRoundedRect(outer, 14, 14)
        p.setFont(theme.app_font(9, 600))
        p.setPen(QColor(theme.MUTED))
        p.drawText(QRectF(16, 10, self.width() - 32, 18), Qt.AlignLeft | Qt.AlignVCenter,
                   (self.title or "PRICE HISTORY").upper())
        r = QRectF(self.rect()).adjusted(58, 38, -18, -28)
        if len(self.points) < 2:
            p.setPen(QColor(theme.FAINT))
            p.setFont(theme.app_font(10))
            p.drawText(self.rect(), Qt.AlignCenter, "Select an item to see its price history")
            return
        vals = self.points + ([self.ref] if self.ref else [])
        lo, hi = min(vals), max(vals)
        pad = (hi - lo) * 0.12 or max(1.0, hi * 0.05)
        lo, hi = lo - pad, hi + pad

        def xy(i, v):
            return (r.left() + r.width() * i / (len(self.points) - 1),
                    r.bottom() - r.height() * (v - lo) / (hi - lo))
        # grid
        p.setFont(theme.app_font(8))
        for k in range(4):
            v = lo + (hi - lo) * k / 3
            _, y = xy(0, v)
            p.setPen(QPen(QColor(theme.BORDER), 1, Qt.DotLine))
            p.drawLine(int(r.left()), int(y), int(r.right()), int(y))
            p.setPen(QColor(theme.FAINT))
            p.drawText(QRectF(4, y - 8, 48, 16), Qt.AlignRight | Qt.AlignVCenter, short(v))
        line = QPainterPath()
        for i, v in enumerate(self.points):
            x, y = xy(i, v)
            line.moveTo(x, y) if i == 0 else line.lineTo(x, y)
        area = QPainterPath(line)
        area.lineTo(r.right(), r.bottom())
        area.lineTo(r.left(), r.bottom())
        area.closeSubpath()
        grad = QLinearGradient(0, r.top(), 0, r.bottom())
        grad.setColorAt(0, QColor(139, 92, 246, 110))
        grad.setColorAt(1, QColor(139, 92, 246, 0))
        p.setPen(Qt.NoPen)
        p.setBrush(grad)
        p.drawPath(area)
        p.setBrush(Qt.NoBrush)
        p.setPen(QPen(QColor(theme.VIOLET), 2.2))
        p.drawPath(line)
        if self.ref:
            _, y = xy(0, self.ref)
            p.setPen(QPen(QColor(theme.GOLD), 1.2, Qt.DashLine))
            p.drawLine(int(r.left()), int(y), int(r.right()), int(y))
            p.setPen(QColor(theme.GOLD))
            p.drawText(QRectF(r.right() - 160, y - 18, 160, 16), Qt.AlignRight, f"usual {short(self.ref)}")
        lx, ly = xy(len(self.points) - 1, self.points[-1])
        p.setPen(Qt.NoPen)
        p.setBrush(QColor(theme.VIOLET))
        p.drawEllipse(QRectF(lx - 4, ly - 4, 8, 8))
        p.setPen(QColor(theme.FAINT))
        p.drawText(QRectF(r.left(), r.bottom() + 6, 80, 16), Qt.AlignLeft, "older")
        p.drawText(QRectF(r.right() - 80, r.bottom() + 6, 80, 16), Qt.AlignRight, "now")
