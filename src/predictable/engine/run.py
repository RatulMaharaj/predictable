import os
from csv import DictReader
from pathlib import Path
from typing import Union

import pandas as pd


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
        # Handle data reading
        if self.modelpoint_file is not None:
            data = DictReader(open(self.modelpoint_file, "r"))

            # Define output location
            output_file = self.results_location / f"{self.label}_results.csv"

            # Delete results if already exists
            if os.path.exists(output_file):
                os.remove(output_file)

            # Loop over rows and convert into a pydantic datclass
            # The data definition is provided by the user
            for row in data:
                modelpoint = self.modelpoint_definition(**row)

                result = self.handler(
                    modelpoint,
                    modelpoint_definition=self.modelpoint_definition,
                    modelpoint_file=self.modelpoint_file,
                    table_location=self.table_location,
                    result_location=self.results_location,
                    runconfig_label=self.label,
                )

                # Convert dict to df
                result = pd.DataFrame.from_dict([result])

                if self.results_location is not None:
                    result.to_csv(
                        output_file,
                        mode="a",
                        index=False,
                        header=not os.path.exists(output_file),
                    )
