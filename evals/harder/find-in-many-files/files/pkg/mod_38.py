from pkg.errors import WorkError


def step_38(value):
    if value < 0:
        raise WorkError("negative")
    return value + 38


def helper_38(items):
    return [step_38(item) for item in items]
