import pandas as pd
import csv
from io import StringIO
from pathlib import Path
from typing import List


def textfile_to_filtered_str_list(
    file_path: str | Path, header_prefix: str, entries_prefix: str, sep: str
) -> List[str]:
    """Converts a textfile to a filtered list of strings

    Parameters
    ----------
    file_path : str | Path
         Path object or string path
    header_prefix : str
        Header marker
    entries_prefix : str
        Entry marker
    sep: str
        Delimiter to use

    Returns
    -------
    List[str]
        Each entry represents a line from the text file
    """
    # initialize results
    result = []
    # keep track of whether or not the header row was processed
    found_header = False

    with open(file_path) as file:
        for _, line in enumerate(file):
            if not line:
                pass  # do nothing if row is empty
            elif (not found_header) and (line.split(sep)[0] == header_prefix):
                found_header = True
                result.append(line)
            elif found_header and (line.split(sep)[0] == entries_prefix):
                result.append(line)

    return result


def str_list_to_stringIO(str_list: List[str], newline="\r\n") -> StringIO:
    """Converts a list of strings to one continuous string, with linebreaks;
    and then wraps that in a stringIO.

    Parameters
    ----------
    str_list : List[str]
        List of strings
    newline : str, optional
        New line character, by default "\r\n"

    Returns
    -------
    StringIO
        StringIO object to be consumed by read_csv
    """
    big_string = newline.join(str_list)
    str_io = StringIO(big_string)
    return str_io


def read_rpt(
    file_path: str | Path,
    header_prefix="!",
    entries_prefix="*",
    sep=",",
    **csv_args,
) -> pd.DataFrame:
    """Read a .rpt file into a pandas DataFrame.

    This is a special text file format where the header row begins with a "!"
    and each data row begins with a "*".

    Parameters
    ----------
    file_path : str
        Path object or string path
    header_prefix : str, optional
        Header marker, by default "!"
    entries_prefix : str, optional
        Entry marker, by default "*"
    sep: str
        Delimiter to use

    Returns
    -------
    pd.DataFrame
        Returns a pandas DataFrame object
    """
    str_list = textfile_to_filtered_str_list(
        file_path, header_prefix, entries_prefix, sep
    )  # reads header (!) and entries (*) only

    str_io = str_list_to_stringIO(
        str_list
    )  # converts list of strings to string_IO needed by pd.read_csv

    df = pd.read_csv(str_io, sep=sep, **csv_args).drop(header_prefix, axis=1)
    return df


def textfile_to_str_list(file_path):
    result = []
    with open(file_path) as file:
        result = [line.strip() for line in file]
    return result


def str_List_to_textfile(str_list):
    blob = "\n".join(str_list)
    return blob


def quote_all_strings(
    x, quote='"'
):  # puts quotes around strings and returns other values unchanged
    return quote + x + quote if isinstance(x, str) else x


# The following extension allows us to do -> df.predictable.to_rpt("file_name.rpt")
# without needing to subclass pd.DataFrame
@pd.api.extensions.register_dataframe_accessor("predictable")
class PredictableAccessor:
    def __init__(self, pandas_obj):
        self._obj = pandas_obj

    def to_rpt(self, file_name: str | None = None, sort_on="SPCODE"):
        # Wrap all strings in quotes
        df = self._obj.applymap(quote_all_strings)

        # Add ! heading with all * entries
        df.insert(0, "!", "*")

        # Sort if required
        if sort_on in list(df.columns):
            df = df.sort_values(sort_on)

        # write to file if file_name is provided
        if file_name:
            return df.to_csv(file_name, index=False, quoting=csv.QUOTE_NONE)
        else:
            return df
