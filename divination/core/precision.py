PRECISION = 2


def set_precision(p: int):
    """Determine the number of decimal places used in rounding.

    Keyword arguments:
    p -- int, number of decimal places
    Return: global PRECISION
    """
    if int(p) > 0:
        global PRECISION
        PRECISION = int(p)
        return PRECISION
    else:
        raise ValueError("Cannot set negative precision.", p)


def get_precision():
    """Get the current number of decimals used in rounding.

    Return: global PRECISION
    """

    return PRECISION
