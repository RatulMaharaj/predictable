from pandas import DataFrame

from .flows import CashFlow, Derived


class Model:
    def __init__(self):
        self.components: dict = {}
        self.results = DataFrame()

    def add_component(self, component: CashFlow):
        self.components[component.label] = component

    def project(self, term: int) -> DataFrame:
        for key in self.components.keys():
            self.results[key] = self.components[key].project(term)
        return self.results

    def derive_component(
        self,
        label: str,
        expression: str,
    ):
        self.components[label] = Derived(label=label, relationship=expression)
        print(expression)
        relationship = expression
        for key in self.components.keys():
            relationship = relationship.replace(
                key, f"self.components['{key}']"
            )

        print(relationship)
        print(eval(relationship))
