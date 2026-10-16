"""Start the GUI."""
from __future__ import annotations

import sys


def main(argv=None) -> int:
    from PySide6.QtWidgets import QApplication

    from .engine import Engine
    from .gui.main_window import MainWindow
    from .gui.theme import apply_theme

    if sys.platform == "win32":
        try:  # own taskbar icon instead of python's
            import ctypes
            ctypes.windll.shell32.SetCurrentProcessExplicitAppUserModelID("KinahForge.Aion2IncomeAssistant")
        except Exception:
            pass
    app = QApplication(sys.argv if argv is None else argv)
    app.setApplicationName("Kinah Forge")
    apply_theme(app)
    win = MainWindow(Engine())
    win.show()
    return app.exec()
