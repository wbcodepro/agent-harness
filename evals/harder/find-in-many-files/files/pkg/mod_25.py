from pkg.errors import WorkError


def step_25(value):
    if value < 0:
        raise WorkError("negative")
    return value + 25


def helper_25(items):
    return [step_25(item) for item in items]
