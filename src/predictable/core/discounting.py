import numpy as np
from numpy.typing import ArrayLike
from pandas import DataFrame

from .flows import StaticFlow


def i_to_v(i: float) -> float:
    """Convert interest rate i to discount factor
    where discount factor = 1 / (1 + interest rate)

    :param i: Interest rate
    :type i: float
    :return: Discount factor
    :rtype: float
    """
    return 1 / (1 + i)


# The TableLookup component can be used instead instead
class DiscountFactors(np.ndarray):
    def __new__(
        cls,
        interest_rate: float,
        input_array: ArrayLike = [1],
        formula=lambda i: i,
        label: str = None,
    ):
        # Input array is an already formed ndarray instance
        obj = np.asarray(input_array).view(cls)
        # add new attributes to the created instance
        obj.interest_rate = interest_rate
        obj.formula = formula
        obj.label = label
        # Finally, we must return the newly created object:
        return obj

    def __array_finalize__(self, obj):
        if obj is None:
            return
        self.interest_rate = getattr(obj, "interest_rate", None)
        self.formula = getattr(obj, "formula", lambda i: i)
        self.label = getattr(obj, "label", None)

    def project(self, term: int, results: DataFrame) -> StaticFlow:
        """This method is used to handle the projection logic for the component.

        :param term: Term over which to project
        :type term: int
        :return: StaticDiscountFactors object containing projected values
        :rtype: StaticDiscountFactors
        """
        results = self
        for n in range(1, term + 1):
            results = np.append(
                results, i_to_v(self.formula(self.interest_rate)) ** n
            )
        return StaticFlow(
            input_array=results,
            label=self.label,
        )
