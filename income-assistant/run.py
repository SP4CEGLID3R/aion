"""Entry point (also used by PyInstaller)."""
import sys

from aion_income.app import main

if __name__ == "__main__":
    sys.exit(main())
