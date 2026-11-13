from pkg.errors import WorkError


def step_33(value):
    if value < 0:
        raise WorkError("negative")
    return value + 33


def helper_33(items):
    return [step_33(item) for item in items]
