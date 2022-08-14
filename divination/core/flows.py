from .precision import get_precision


class StaticFlow:
    def __init__(self, label: str, data: list):
        self.label = label
        self.data = data
        self.length = len(data)

    def __str__(self):
        return f"{self.data}"

    def project(self, term: int):
        if self.length == term:
            return self.data
        elif self.length < term:
            return self.data + (term - self.length + 1) * [0]
        elif self.length > term:
            return self.data[: term + 1]


class CashFlow:
    def __init__(
        self, label: str, initial_data: list, formula=lambda previous: previous
    ):
        self.label = label
        self.formula = formula
        self.initial_data = initial_data
        self.data = initial_data
        self.length = len(initial_data)

    def __str__(self):
        return f"{self.data}"

    def project(self, term):
        for _ in range(0, term):
            self.data.append(
                round(
                    self.formula(self.data[-1]),
                    get_precision(),
                )
            )
        self.length = len(self.data)
        return self.data
