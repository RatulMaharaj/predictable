import os
from csv import DictReader
from pathlib import Path
from typing import Union


# TODO: Extend to be able to work with various input formats
class RunConfig:
    instances = []

    def __init__(
        self,
        handler,
        modelpoint_definition,
        modelpoint_file: Union[str, Path] = None,
        table_location: Union[str, Path] = None,
        results_location: Union[str, Path] = None,
        label: str = None,
    ) -> None:
        if modelpoint_file is not None:
            if not os.path.exists(modelpoint_file):
                raise FileNotFoundError(modelpoint_file)
            else:
                self.modelpoint_file = Path(modelpoint_file)

        if table_location is not None:
            if not os.path.exists(table_location):
                raise FileNotFoundError(table_location)
            else:
                self.table_location = Path(table_location)

        if results_location is not None:
            if not os.path.exists(results_location):
                os.makedirs(results_location)

            self.results_location = Path(results_location)

        self.handler = handler
        self.modelpoint_definition = modelpoint_definition
        self.label = label

        RunConfig.instances.append(self)

    def __repr__(self) -> str:
        return f"RunConfig(label='{self.label}')"

    @classmethod
    def run_all(cls):
        for run_setting in cls.instances:
            run_setting.run()

    def run(self):
        # handle data reading
        if self.modelpoint_file is not None:
            data = DictReader(open(self.modelpoint_file, "r"))
            # loop over rows and convert into a pydantic datclass
            # the data definition is provided by the user

            for row in data:
                modelpoint = self.modelpoint_definition(**row)
                result = self.handler(
                    modelpoint, table_location=self.table_location
                )

                # output the results to provided location
                if self.results_location is not None:
                    pass

                # for now just print results to screen
                print(result)
