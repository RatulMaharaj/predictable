import os
from csv import DictReader
from pathlib import Path
from typing import Union


class RunSetting:
    def __init__(
        self,
        handler,
        modelpoint_definition,
        modelpoint_file: Union[str, Path],
        results_location: Union[str, Path],
    ) -> None:
        # Perform some validation before creating the instance
        if not os.path.exists(modelpoint_file):
            raise FileNotFoundError(modelpoint_file)

        if not os.path.exists(results_location):
            os.makedirs(results_location)

        self.handler = handler
        self.modelpoint_definition = modelpoint_definition
        self.modelpoint_file = Path(modelpoint_file)
        self.results_location = Path(results_location)

    def run(self):
        # handle data reading
        data = DictReader(open(self.modelpoint_file, "r"))
        # loop over rows and convert into a pydantic datclass
        # the data definition is provided by the user
        for row in data:
            modelpoint = self.modelpoint_definition(**row)
            result = self.handler(modelpoint)
            # for now just print results to screen
            print(result)
