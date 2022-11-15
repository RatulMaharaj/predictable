from typing import List

from numpy import arange
from pandas import DataFrame

from .flows import CashFlow


class Model:
    """Basic building block of the core modelling library."""

    def __init__(self):
        self.components: dict = {}
        self.results = DataFrame()

    def add_component(self, component: CashFlow):
        """This method is used to associate a component with the Model instance.

        :param component: Component to be added to the Model instance
        :type component: CashFlow
        """
        self.components[component.label] = component

    def add_components(self, components: List):
        """This method is used to associate a list of components with the Model instance.

        :param components: List of component to be added to the Model instance
        :type components: List
        """
        for component in components:
            self.components[component.label] = component

    def project(self, term: int) -> DataFrame:
        """This method is used to invoke the project method in each of the associated components.

        :param term: The term over which to project
        :type term: int
        :return: Results of each components project call
        :rtype: DataFrame
        """
        # Create time index
        self.results["t"] = arange(0, term + 1)

        # Call project method for each component
        for key in self.components.keys():
            self.results[key] = self.components[key].project(
                term=term,
                results=self.results,
            )
        return self.results
