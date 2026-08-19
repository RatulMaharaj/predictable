"""The library's mortality module: a flat loading a product may specialise, and two it may not."""

from predictable import final, series
from predictable.timing import END
from predictable.units import Factor


@series(timing=END)
def qx_loading() -> Factor:
    """The library's flat loading. Products with select experience override this."""
    return 1.0


@final
@series(timing=END)
def statutory_margin() -> Factor:
    """Group standard — every product computes this the same way."""
    return 1.05
