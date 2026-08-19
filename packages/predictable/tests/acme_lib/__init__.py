"""A stand-in for an in-company library package (`02-dsl.md` §7.1).

The real thing is an ordinary Python distribution with a ``predictable.library`` entry point; what
matters for the declaration layer is only that a :class:`~predictable.Library` names its modules,
so a product can `extends()` them and the resolved version can be recorded in a run manifest.
"""

from predictable import Library

LIBRARY = Library(name="acme", version="3.2.0", modules=["acme_lib.mortality"])
