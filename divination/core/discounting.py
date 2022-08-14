def i_to_v(i: float) -> float:
    """Convert interest rate i to discount factor v = 1 / (1 + i).

    Keyword arguments:
    i -- interest rate
    Return: v, discount factor
    """

    return 1 / (1 + i)


class StaticDiscountFactors:
    def __init__(self, data: list, label: str = "discount_factor"):
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


class DiscountFactors:
    def __init__(
        self,
        interest_rate: float,
        formula=lambda i: i,
        label: str = "discount_factor",
    ):
        self.label = label
        self.interest_rate = interest_rate
        self.formula = formula
        self.data = [1]
        self.length = len(self.data)

    def __str__(self):
        return f"{self.data}"

    def project(self, term):
        for n in range(1, term + 1):
            self.data.append(i_to_v(self.formula(self.interest_rate)) ** n)
        self.length = len(self.data)
        return self.data
