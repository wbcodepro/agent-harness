from pkg.errors import WorkError


def step_11(value):
    if value < 0:
        raise WorkError("negative")
    return value + 11


def helper_11(items):
    return [step_11(item) for item in items]
