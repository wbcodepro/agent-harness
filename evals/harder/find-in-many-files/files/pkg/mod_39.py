from pkg.errors import WorkError


def step_39(value):
    if value < 0:
        raise WorkError("negative")
    return value + 39


def helper_39(items):
    return [step_39(item) for item in items]
