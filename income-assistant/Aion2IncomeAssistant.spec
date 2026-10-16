# PyInstaller spec: one-file windowed executable.  Build with:
#   pyinstaller Aion2IncomeAssistant.spec
a = Analysis(
    ["run.py"],
    pathex=["."],
    datas=[("aion_income/data", "aion_income/data")],
    excludes=["tkinter", "PySide6.QtWebEngineCore", "PySide6.QtWebEngineWidgets", "PySide6.Qt3DCore",
              "PySide6.QtQuick", "PySide6.QtQml", "PySide6.QtMultimedia", "PySide6.QtCharts"],
)
pyz = PYZ(a.pure)
exe = EXE(
    pyz,
    a.scripts,
    a.binaries,
    a.datas,
    name="Aion2IncomeAssistant",
    console=False,
    upx=False,
)
