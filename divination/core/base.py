import numpy as np
from numpy.typing import ArrayLike


class BaseFlow(np.ndarray):
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
        if self.length == term:
            return self.data
        elif self.length < term:
            return self.data + (term - self.length + 1) * [0]
        elif self.length > term:
            return self.data[: term + 1]
