import numpy as np
from numpy.typing import ArrayLike

from .base import BaseFlow


def i_to_v(i: float) -> float:
    """Convert interest rate i to discount factor v = 1 / (1 + i)."""
    return 1 / (1 + i)


# TODO: A lot of the class setup logic is being repeated
# TODO: We should create a BaseFlow class for other Flows to inherit from
class StaticDiscountFactors(np.ndarray):
    def __new__(cls, input_array: ArrayLike, label: str = None):
        # Input array is an already formed ndarray instance
        # We first cast to be our class type
        obj = np.asarray(input_array).view(cls)
        # add new attributes to the created instance
        obj.label = label
        # Finally, we must return the newly created object:
        return obj

    def __array_finalize__(self, obj):
        if obj is None:
            return
        self.label = getattr(obj, "label", None)

    def project(self, term: int):
        if len(self) == term:
            return self
        elif len(self) < term:
            results = np.append(self, (term - len(self) + 1) * [0])
            return StaticDiscountFactors(input_array=results, label=self.label)
        elif len(self) > term:
            results = self[: term + 1]
            return StaticDiscountFactors(input_array=results, label=self.label)


class DiscountFactors(BaseFlow):
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

    def project(self, term):
        results = self
        for n in range(1, term + 1):
            results = np.append(
                results, i_to_v(self.formula(self.interest_rate)) ** n
            )
        return StaticDiscountFactors(
            input_array=results,
            label=self.label,
        )
