"""Render the app logo to icon.ico (used by PyInstaller for the .exe)."""
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtGui import QGuiApplication  # noqa: E402

from aion_income.gui.theme import logo_pixmap  # noqa: E402

app = QGuiApplication([])
out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "icon.ico")
ok = logo_pixmap(256).save(out, "ICO")
print("icon.ico" if ok else "ICO writer not available")
sys.exit(0 if ok else 1)
