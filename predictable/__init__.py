__version__ = "v0.0.2"

# External imports
from pydantic.dataclasses import dataclass

# Core imports
from .core.discounting import DiscountFactors, i_to_v
from .core.flows import CashFlow, StaticCashFlow
from .core.lookup import TableLookup
from .core.model import Model
from .core.precision import PRECISION, get_precision, set_precision
from .core.rating_factors import RatingFactor, StaticRatingFactor
from .core.io import read_rpt

# Pandas imports (for convenience)
from pandas import DataFrame, read_csv, read_excel, read_sql, read_json

# Engine imports
from .engine.run import RunConfig

__all__ = [
    "dataclass",
    "DiscountFactors",
    "i_to_v",
    "CashFlow",
    "StaticCashFlow",
    "TableLookup",
    "Model",
    "PRECISION",
    "get_precision",
    "set_precision",
    "RatingFactor",
    "StaticRatingFactor",
    "read_rpt",
    "RunConfig",
    "DataFrame",
    "read_csv",
    "read_excel",
    "read_sql",
    "read_json",
]
