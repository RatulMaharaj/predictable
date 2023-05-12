__version__ = "v0.0.0-alpha"

# External imports
from pydantic.dataclasses import dataclass

# Core imports
from .core.discounting import DiscountFactors, i_to_v
from .core.flows import CashFlow, StaticFlow
from .core.lookup import TableLookup
from .core.model import Model
from .core.precision import PRECISION, get_precision, set_precision
from .core.rating_factors import RatingFactor, StaticRatingFactor
from .core.io import read_rpt

# Engine imports
from .engine.run import RunConfig

__all__ = [
    "dataclass",
    "DiscountFactors",
    "i_to_v",
    "CashFlow",
    "StaticFlow",
    "TableLookup",
    "Model",
    "PRECISION",
    "get_precision",
    "set_precision",
    "RatingFactor",
    "StaticRatingFactor",
    "read_rpt",
    "RunConfig",
]
