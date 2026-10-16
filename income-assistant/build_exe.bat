@echo off
rem Build dist\Aion2IncomeAssistant.exe
cd /d "%~dp0"
if not exist .venv (
    py -3 -m venv .venv || python -m venv .venv
)
.venv\Scripts\python -m pip install -r requirements-dev.txt
.venv\Scripts\python -m pytest -q tests || exit /b 1
.venv\Scripts\pyinstaller --noconfirm Aion2IncomeAssistant.spec
echo.
echo Built dist\Aion2IncomeAssistant.exe
