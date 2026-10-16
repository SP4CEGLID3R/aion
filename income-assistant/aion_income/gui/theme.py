"""Dark theme: colour tokens, the global stylesheet, painted icons and the app logo."""
from __future__ import annotations

import math
import os
import sys
import tempfile

from PySide6.QtCore import QPointF, QRectF, Qt
from PySide6.QtGui import (
    QColor, QFont, QIcon, QLinearGradient, QPainter, QPainterPath, QPalette, QPen, QPixmap, QRadialGradient,
)

# ── colour tokens ────────────────────────────────────────────────────────
BG = "#0b0d12"          # window
SIDEBAR = "#0f1218"
SURFACE = "#141821"     # cards, tables
SURFACE_2 = "#1a1f2b"   # inputs, hovered rows
SURFACE_3 = "#232a39"
BORDER = "#252c3b"
TEXT = "#e8ebf2"
MUTED = "#8b93a7"
FAINT = "#5d6577"
GOLD = "#f2b84b"        # kinah
GOLD_2 = "#d4932a"
VIOLET = "#8b5cf6"      # Asmodian accent
VIOLET_SOFT = "#2a2347"
GREEN = "#34d399"
AMBER = "#fbbf24"
RED = "#f87171"
BLUE = "#60a5fa"

RISK_COLORS = {"Safe": GREEN, "Moderate": AMBER, "Risky": RED}

FONT_FAMILIES = ["Segoe UI Variable Text", "Segoe UI", "Inter", "Noto Sans", "DejaVu Sans"]


def app_font(size: float = 10.0, weight: int = 400) -> QFont:
    f = QFont()
    f.setFamilies(FONT_FAMILIES)
    f.setPointSizeF(size)
    f.setWeight(QFont.Weight(int(weight)))
    return f


STYLESHEET = f"""
* {{ outline: none; }}
QMainWindow, QWidget#Root {{ background: {BG}; }}
QWidget {{ color: {TEXT}; font-size: 10pt; }}
QLabel {{ background: transparent; }}
QLabel#Muted, QLabel[muted="true"] {{ color: {MUTED}; }}
QLabel#PageTitle {{ font-size: 19pt; font-weight: 600; color: {TEXT}; }}
QLabel#PageSubtitle {{ color: {MUTED}; font-size: 10pt; }}
QLabel#SectionTitle {{ color: {MUTED}; font-size: 8.5pt; font-weight: 600; letter-spacing: 1px; }}

/* sidebar */
QFrame#Sidebar {{ background: {SIDEBAR}; border-right: 1px solid {BORDER}; }}
QLabel#Brand {{ font-size: 13pt; font-weight: 700; color: {TEXT}; }}
QLabel#BrandSub {{ color: {MUTED}; font-size: 8.5pt; }}
QPushButton#Nav {{
    text-align: left; padding: 10px 14px; border: none; border-radius: 10px;
    color: {MUTED}; background: transparent; font-size: 10.5pt; font-weight: 500;
}}
QPushButton#Nav:hover {{ background: {SURFACE}; color: {TEXT}; }}
QPushButton#Nav:checked {{ background: {VIOLET_SOFT}; color: {TEXT}; font-weight: 600; }}
QFrame#ServerPill {{ background: {SURFACE}; border: 1px solid {BORDER}; border-radius: 12px; }}

/* cards */
QFrame#Card {{ background: {SURFACE}; border: 1px solid {BORDER}; border-radius: 14px; }}
QFrame#KpiCard {{ background: {SURFACE}; border: 1px solid {BORDER}; border-radius: 14px; }}
QLabel#KpiTitle {{ color: {MUTED}; font-size: 8.5pt; font-weight: 600; letter-spacing: 0.5px; }}
QLabel#KpiValue {{ font-size: 18pt; font-weight: 650; }}
QLabel#KpiHint {{ color: {FAINT}; font-size: 8.5pt; }}
QFrame#TierCard {{ background: {SURFACE}; border: 1px solid {BORDER}; border-radius: 14px; }}
QFrame#TierCard[accent="true"] {{ border: 1px solid {GOLD_2}; }}
QLabel#Banner {{
    background: #2a2210; color: #f5d78e; border: 1px solid #4a3a14; border-radius: 10px; padding: 9px 12px;
}}

/* buttons */
QPushButton {{
    background: {SURFACE_2}; border: 1px solid {BORDER}; border-radius: 9px; padding: 7px 14px; color: {TEXT};
}}
QPushButton:hover {{ background: {SURFACE_3}; }}
QPushButton:disabled {{ color: {FAINT}; }}
QPushButton#Primary {{
    background: qlineargradient(x1:0, y1:0, x2:1, y2:1, stop:0 {GOLD}, stop:1 {GOLD_2});
    color: #1a1204; border: none; font-weight: 700; padding: 9px 20px; border-radius: 10px;
}}
QPushButton#Primary:hover {{ background: {GOLD}; }}
QPushButton#Primary:disabled {{ background: {SURFACE_3}; color: {MUTED}; }}

/* inputs */
QLineEdit, QComboBox, QSpinBox, QDoubleSpinBox {{
    background: {SURFACE_2}; border: 1px solid {BORDER}; border-radius: 9px; padding: 5px 10px;
    selection-background-color: {VIOLET}; color: {TEXT}; min-height: 22px;
}}
QLineEdit:focus, QComboBox:focus, QSpinBox:focus, QDoubleSpinBox:focus {{ border: 1px solid {VIOLET}; }}
QLineEdit[invalid="true"] {{ border: 1px solid {RED}; }}
QComboBox {{ padding-right: 28px; }}
QComboBox::drop-down {{ border: none; width: 26px; subcontrol-position: center right; }}
QComboBox::down-arrow {{ image: url(@ASSETS@/chevron_down.png); width: 12px; height: 12px; }}
QComboBox QAbstractItemView {{
    background: {SURFACE_2}; border: 1px solid {BORDER}; selection-background-color: {VIOLET_SOFT}; padding: 4px;
}}
QSpinBox, QDoubleSpinBox {{ padding-right: 24px; }}
QSpinBox::up-button, QDoubleSpinBox::up-button {{
    subcontrol-origin: border; subcontrol-position: top right; width: 22px; border: none; background: transparent;
}}
QSpinBox::down-button, QDoubleSpinBox::down-button {{
    subcontrol-origin: border; subcontrol-position: bottom right; width: 22px; border: none; background: transparent;
}}
QSpinBox::up-button:hover, QDoubleSpinBox::up-button:hover,
QSpinBox::down-button:hover, QDoubleSpinBox::down-button:hover {{ background: {SURFACE_3}; border-radius: 6px; }}
QSpinBox::up-arrow, QDoubleSpinBox::up-arrow {{ image: url(@ASSETS@/chevron_up.png); width: 10px; height: 10px; }}
QSpinBox::down-arrow, QDoubleSpinBox::down-arrow {{ image: url(@ASSETS@/chevron_down.png); width: 10px; height: 10px; }}

/* chips (checkboxes) */
QCheckBox {{ spacing: 7px; color: {TEXT}; }}
QCheckBox::indicator {{
    width: 16px; height: 16px; border-radius: 5px; border: 1px solid {FAINT}; background: {SURFACE_2};
}}
QCheckBox::indicator:checked {{ background: {VIOLET}; border: 1px solid {VIOLET}; image: url(@ASSETS@/check.png); }}
QCheckBox#Chip {{
    background: {SURFACE}; border: 1px solid {BORDER}; border-radius: 15px; padding: 5px 12px 5px 9px;
}}
QCheckBox#Chip:checked {{ background: {SURFACE_2}; border: 1px solid {SURFACE_3}; }}

/* tables */
QTableView {{
    background: {SURFACE}; alternate-background-color: #161a24; border: 1px solid {BORDER}; border-radius: 14px;
    gridline-color: transparent; selection-background-color: {VIOLET_SOFT}; selection-color: {TEXT}; padding: 4px;
}}
QTableView::item {{ padding: 0 8px; border: none; }}
QTableView::item:hover {{ background: {SURFACE_2}; }}
QTableView::item:selected {{ background: {VIOLET_SOFT}; }}
QHeaderView {{ background: transparent; }}
QHeaderView::section {{
    background: {SURFACE}; color: {MUTED}; border: none; border-bottom: 1px solid {BORDER};
    padding: 8px 8px; font-size: 8.5pt; font-weight: 600;
}}
QHeaderView::section:hover {{ color: {TEXT}; }}
QTableCornerButton::section {{ background: {SURFACE}; border: none; }}

QTextBrowser {{
    background: {SURFACE}; border: 1px solid {BORDER}; border-radius: 14px; padding: 10px 14px;
}}
QSplitter::handle {{ background: transparent; }}
QSplitter::handle:vertical {{ height: 10px; }}
QSplitter::handle:horizontal {{ width: 10px; }}

QProgressBar {{ background: {SURFACE_2}; border: none; border-radius: 3px; max-height: 6px; min-height: 6px; }}
QProgressBar::chunk {{
    border-radius: 3px; background: qlineargradient(x1:0, y1:0, x2:1, y2:0, stop:0 {VIOLET}, stop:1 {GOLD});
}}

QScrollBar:vertical {{ background: transparent; width: 10px; margin: 4px 2px; }}
QScrollBar::handle:vertical {{ background: {SURFACE_3}; border-radius: 3px; min-height: 30px; }}
QScrollBar::handle:vertical:hover {{ background: {FAINT}; }}
QScrollBar:horizontal {{ background: transparent; height: 10px; margin: 2px 4px; }}
QScrollBar::handle:horizontal {{ background: {SURFACE_3}; border-radius: 3px; min-width: 30px; }}
QScrollBar::add-line, QScrollBar::sub-line {{ width: 0; height: 0; }}
QScrollBar::add-page, QScrollBar::sub-page {{ background: transparent; }}

QToolTip {{
    background: {SURFACE_3}; color: {TEXT}; border: 1px solid {BORDER}; border-radius: 6px; padding: 6px 8px;
}}
QMessageBox {{ background: {SURFACE}; }}
"""

# HTML in the detail panels (QTextBrowser supports a CSS subset)
DOC_CSS = f"""
body {{ color: {TEXT}; font-size: 10pt; }}
h2 {{ color: {TEXT}; font-size: 15pt; font-weight: 600; margin: 0 0 2px 0; }}
.muted {{ color: {MUTED}; }}
.gold {{ color: {GOLD}; font-weight: 600; }}
.green {{ color: {GREEN}; }}
.red {{ color: {RED}; }}
.amber {{ color: {AMBER}; }}
th {{ color: {MUTED}; font-size: 8.5pt; font-weight: 600; text-align: left; padding: 4px 10px 4px 0; }}
td {{ padding: 4px 10px 4px 0; }}
a {{ color: {BLUE}; }}
"""


def apply_theme(app) -> None:
    app.setStyle("Fusion")
    app.setFont(app_font(10))
    pal = QPalette()
    for role, c in ((QPalette.Window, BG), (QPalette.WindowText, TEXT), (QPalette.Base, SURFACE),
                    (QPalette.AlternateBase, "#161a24"), (QPalette.Text, TEXT), (QPalette.Button, SURFACE_2),
                    (QPalette.ButtonText, TEXT), (QPalette.Highlight, VIOLET), (QPalette.HighlightedText, "#ffffff"),
                    (QPalette.ToolTipBase, SURFACE_3), (QPalette.ToolTipText, TEXT), (QPalette.PlaceholderText, FAINT),
                    (QPalette.Mid, BORDER), (QPalette.Link, BLUE)):
        pal.setColor(role, QColor(c))
    app.setPalette(pal)
    app.setStyleSheet(STYLESHEET.replace("@ASSETS@", ensure_assets()))
    app.setWindowIcon(app_icon())


def tint(color: str, alpha: float, over: str = SURFACE) -> str:
    """Solid colour of `color` at `alpha` over the card background (rich text has no alpha)."""
    a, b = QColor(color), QColor(over)
    mix = [round(x * alpha + y * (1 - alpha)) for x, y in ((a.red(), b.red()), (a.green(), b.green()),
                                                           (a.blue(), b.blue()))]
    return "#%02x%02x%02x" % tuple(mix)


def ensure_assets() -> str:
    """Draw the small stylesheet images (arrows, tick) once and return their folder."""
    folder = os.path.join(tempfile.gettempdir(), "kinah_forge_ui_v1")
    os.makedirs(folder, exist_ok=True)

    def draw(name, fn, color):
        path = os.path.join(folder, name)
        if os.path.exists(path):
            return
        pm = QPixmap(48, 48)
        pm.fill(Qt.transparent)
        p = QPainter(pm)
        p.setRenderHint(QPainter.Antialiasing)
        p.setPen(QPen(QColor(color), 6, Qt.SolidLine, Qt.RoundCap, Qt.RoundJoin))
        fn(p)
        p.end()
        pm.save(path, "PNG")
    draw("chevron_down.png", lambda p: p.drawPolyline([QPointF(10, 17), QPointF(24, 31), QPointF(38, 17)]), MUTED)
    draw("chevron_up.png", lambda p: p.drawPolyline([QPointF(10, 31), QPointF(24, 17), QPointF(38, 31)]), MUTED)
    draw("check.png", lambda p: p.drawPolyline([QPointF(11, 25), QPointF(20, 34), QPointF(37, 15)]), "#ffffff")
    return folder.replace("\\", "/")


def dark_title_bar(widget) -> None:
    """Windows 10/11: ask the window manager for a dark title bar (no effect elsewhere)."""
    if sys.platform != "win32":
        return
    try:
        import ctypes
        hwnd = int(widget.winId())
        value = ctypes.c_int(1)
        for attr in (20, 19):  # DWMWA_USE_IMMERSIVE_DARK_MODE (new / old builds)
            if ctypes.windll.dwmapi.DwmSetWindowAttribute(hwnd, attr, ctypes.byref(value), ctypes.sizeof(value)) == 0:
                break
        caption = ctypes.c_int(0x00120F0B)  # DWMWA_CAPTION_COLOR (Win 11), BGR of the sidebar colour
        ctypes.windll.dwmapi.DwmSetWindowAttribute(hwnd, 35, ctypes.byref(caption), ctypes.sizeof(caption))
    except Exception:
        pass


# ── painted logo and icons (no image files needed) ───────────────────────

def paint_logo(p: QPainter, rect: QRectF) -> None:
    """A gold kinah coin with a violet rune ring."""
    p.save()
    p.setRenderHint(QPainter.Antialiasing)
    c = rect.center()
    r = min(rect.width(), rect.height()) / 2
    glow = QRadialGradient(c, r)
    glow.setColorAt(0.0, QColor(139, 92, 246, 120))
    glow.setColorAt(1.0, QColor(139, 92, 246, 0))
    p.setPen(Qt.NoPen)
    p.setBrush(glow)
    p.drawEllipse(c, r, r)
    coin = QLinearGradient(rect.topLeft(), rect.bottomRight())
    coin.setColorAt(0.0, QColor("#ffd977"))
    coin.setColorAt(0.5, QColor(GOLD))
    coin.setColorAt(1.0, QColor("#a8681a"))
    p.setBrush(coin)
    p.setPen(QPen(QColor("#fff1c2"), max(1.0, r * 0.05)))
    p.drawEllipse(c, r * 0.78, r * 0.78)
    p.setPen(QPen(QColor(120, 70, 10, 160), max(1.0, r * 0.06)))
    p.setBrush(Qt.NoBrush)
    p.drawEllipse(c, r * 0.6, r * 0.6)
    # stylised "K" made of two strokes and a diamond
    pen = QPen(QColor("#4a2a05"), max(1.5, r * 0.13), Qt.SolidLine, Qt.RoundCap, Qt.RoundJoin)
    p.setPen(pen)
    x0 = c.x() - r * 0.2
    p.drawLine(QPointF(x0, c.y() - r * 0.33), QPointF(x0, c.y() + r * 0.33))
    p.drawLine(QPointF(x0 + r * 0.04, c.y()), QPointF(c.x() + r * 0.24, c.y() - r * 0.33))
    p.drawLine(QPointF(x0 + r * 0.04, c.y()), QPointF(c.x() + r * 0.24, c.y() + r * 0.33))
    p.restore()


def logo_pixmap(size: int) -> QPixmap:
    pm = QPixmap(size, size)
    pm.fill(Qt.transparent)
    p = QPainter(pm)
    paint_logo(p, QRectF(0, 0, size, size))
    p.end()
    return pm


def app_icon() -> QIcon:
    icon = QIcon()
    for s in (16, 24, 32, 48, 64, 128, 256):
        icon.addPixmap(logo_pixmap(s))
    return icon


def _icon_path(name: str, s: float) -> QPainterPath:
    path = QPainterPath()
    if name == "earn":  # rising line with arrow
        path.moveTo(s * 0.12, s * 0.75)
        path.lineTo(s * 0.4, s * 0.48)
        path.lineTo(s * 0.58, s * 0.62)
        path.lineTo(s * 0.88, s * 0.28)
        path.moveTo(s * 0.66, s * 0.26)
        path.lineTo(s * 0.88, s * 0.28)
        path.lineTo(s * 0.86, s * 0.5)
    elif name == "plan":  # stacked coins
        for i in range(3):
            y = s * (0.72 - i * 0.2)
            path.addRoundedRect(QRectF(s * 0.18, y - s * 0.08, s * 0.64, s * 0.16), s * 0.08, s * 0.08)
    elif name == "craft":  # hammer
        path.addRoundedRect(QRectF(s * 0.22, s * 0.16, s * 0.5, s * 0.2), s * 0.04, s * 0.04)
        path.moveTo(s * 0.47, s * 0.36)
        path.lineTo(s * 0.47, s * 0.86)
    elif name == "market":  # bars
        for i, h in enumerate((0.35, 0.6, 0.45, 0.75)):
            x = s * (0.16 + i * 0.19)
            path.addRoundedRect(QRectF(x, s * (0.86 - h), s * 0.12, s * h), s * 0.03, s * 0.03)
    elif name == "supply":  # crate
        path.addRoundedRect(QRectF(s * 0.16, s * 0.3, s * 0.68, s * 0.52), s * 0.06, s * 0.06)
        path.moveTo(s * 0.16, s * 0.48)
        path.lineTo(s * 0.84, s * 0.48)
        path.moveTo(s * 0.5, s * 0.3)
        path.lineTo(s * 0.5, s * 0.16)
    elif name == "settings":  # gear
        cx = cy = s / 2
        teeth = 8
        for i in range(teeth * 2):
            a = math.pi * 2 * i / (teeth * 2)
            rr = s * (0.4 if i % 2 == 0 else 0.3)
            pt = QPointF(cx + rr * math.cos(a), cy + rr * math.sin(a))
            path.moveTo(pt) if i == 0 else path.lineTo(pt)
        path.closeSubpath()
        path.addEllipse(QPointF(cx, cy), s * 0.12, s * 0.12)
    elif name == "refresh":
        path.arcMoveTo(QRectF(s * 0.18, s * 0.18, s * 0.64, s * 0.64), 60)
        path.arcTo(QRectF(s * 0.18, s * 0.18, s * 0.64, s * 0.64), 60, 280)
        path.moveTo(s * 0.66, s * 0.12)
        path.lineTo(s * 0.68, s * 0.33)
        path.lineTo(s * 0.47, s * 0.31)
    return path


def nav_icon(name: str, color: str = MUTED, size: int = 20) -> QIcon:
    icon = QIcon()
    for scale in (1, 2):
        s = size * scale
        pm = QPixmap(s, s)
        pm.fill(Qt.transparent)
        p = QPainter(pm)
        p.setRenderHint(QPainter.Antialiasing)
        p.setPen(QPen(QColor(color), max(1.6, s * 0.09), Qt.SolidLine, Qt.RoundCap, Qt.RoundJoin))
        p.setBrush(Qt.NoBrush)
        p.drawPath(_icon_path(name, s))
        p.end()
        pm.setDevicePixelRatio(scale)
        icon.addPixmap(pm)
    return icon
