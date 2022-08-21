from pathlib import Path
from typing import Any, List

import numpy as np
from pandas import DataFrame, read_csv

from .flows import StaticFlow


class TableLookup(np.ndarray):
    """RiskRates object created from an Array-Like object. Subclasses numpy.ndarray"""

    def __new__(
        cls,
        table_name: str,
        lookup_on: List = [],
        keep_column: str = None,
        default_value: Any = None,
        label: str = None,
    ):
        # Input array is an already formed ndarray instance
        obj = np.asarray([]).view(cls)
        # add new attributes to the created instance
        obj.table_name = table_name
        obj.default_value = default_value
        obj.label = label

        if keep_column is None:
            raise ValueError("Provide a column from the table to be kept.")
        else:
            obj.keep_column = keep_column

        if len(lookup_on) == 0:
            raise ValueError("No lookup column index provided.")
        else:
            obj.lookup_on = lookup_on
        # Finally, we must return the newly created object:
        return obj

    def __array_finalize__(self, obj):
        if obj is None:
            return
        self.table_name = getattr(obj, "table_name", None)
        self.label = getattr(obj, "label", None)
        self.lookup_on = getattr(obj, "lookup_on", [])
        self.keep_column = getattr(obj, "keep_column", [])
        self.default_value = getattr(obj, "default_value", [])

    def project(self, term: int, results: DataFrame) -> StaticFlow:
        """This method is used to handle the projection logic for the component.

        :param term: Term over which to project
        :type term: int
        :return: StaticFlow object containing projected values
        :rtype: StaticFlow
        """
        table = read_csv(Path(self.table_name))

        # join the table and the results
        results = results.join(
            table, on=self.lookup_on, how="left", rsuffix="_table"
        )[self.keep_column]

        # Optionally handle nulls
        if self.default_value is not None:
            results = results.replace(np.NaN, self.default_value)

        return StaticFlow(input_array=results, label=self.label)
