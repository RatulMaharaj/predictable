"""``python -m predictable build models/`` — see :mod:`predictable.cli`."""

import sys

from .cli import main

if __name__ == "__main__":
    sys.exit(main())
