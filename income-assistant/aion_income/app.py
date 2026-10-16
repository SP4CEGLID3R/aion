"""Start the GUI."""
from __future__ import annotations

import sys


def main(argv=None) -> int:
    from PySide6.QtWidgets import QApplication

    from .engine import Engine
    from .gui.main_window import MainWindow

    app = QApplication(sys.argv if argv is None else argv)
    app.setApplicationName("Aion 2 Income Assistant")
    app.setStyle("Fusion")
    win = MainWindow(Engine())
    win.show()
    return app.exec()
