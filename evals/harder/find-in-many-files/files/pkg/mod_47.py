from pkg.errors import WorkError


def step_47(value):
    if value < 0:
        raise WorkError("negative")
    return value + 47


def helper_47(items):
    return [step_47(item) for item in items]
