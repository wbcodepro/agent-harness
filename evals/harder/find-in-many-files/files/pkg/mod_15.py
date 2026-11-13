from pkg.errors import WorkError


def step_15(value):
    if value < 0:
        raise WorkError("negative")
    return value + 15


def helper_15(items):
    return [step_15(item) for item in items]
