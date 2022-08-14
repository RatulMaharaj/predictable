from pandas import DataFrame

from .flows import CashFlow


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
