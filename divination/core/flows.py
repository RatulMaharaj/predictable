import numpy as np
from numpy.typing import ArrayLike

from .precision import get_precision


class StaticFlow(np.ndarray):
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
            return StaticFlow(input_array=results, label=self.label)
        elif len(self) > term:
            results = self[: term + 1]
            return StaticFlow(input_array=results, label=self.label)


class CashFlow(np.ndarray):
    def __new__(
        cls,
        input_array: ArrayLike,
        formula=lambda previous: previous,
        label: str = None,
    ):
        # Input array is an already formed ndarray instance
        obj = np.asarray(input_array).view(cls)
        # add new attributes to the created instance
        obj.label = label
        obj.formula = formula
        # Finally, we must return the newly created object:
        if len(input_array) > 1:
            raise ValueError(
                "Use StaticFlow instead if you wish to pre-populate cashflows.",
                input_array,
            )
        return obj

    def __array_finalize__(self, obj):
        if obj is None:
            return
        self.label = getattr(obj, "label", None)
        self.formula = getattr(obj, "formula", lambda x: x)

    def project(self, term):
        results = self
        for _ in range(0, term):
            results = np.append(
                results,
                round(
                    self.formula(results[-1]),
                    get_precision(),
                ),
            )
        return StaticFlow(input_array=results, label=self.label)


class Derived(np.ndarray):
    def __init__(self, label: str, relationship: str):
        super().__init__(label=label, input_array=[])
        self.relationship = relationship
