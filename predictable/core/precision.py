PRECISION = 2


def set_precision(p: int) -> int:
    """Determine the number of decimal places used in rounding.

    :param p: Number of decimal places to round to, default is 2.
    :type p: int
    :raises ValueError: When attempting to set negative precision.
    :return: global PRECISION
    :rtype: int
    """

    if int(p) > 0:
        global PRECISION
        PRECISION = int(p)
        return PRECISION
    else:
        raise ValueError("Cannot set negative precision.", p)


def get_precision() -> int:
    """Get the current number of decimals used in rounding.

    :return: global PRECISION
    :rtype: int
    """

    return PRECISION
