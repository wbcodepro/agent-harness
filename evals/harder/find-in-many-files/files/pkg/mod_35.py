from pkg.errors import WorkError


def step_35(value):
    if value < 0:
        raise WorkError("negative")
    return value + 35


def helper_35(items):
    return [step_35(item) for item in items]
