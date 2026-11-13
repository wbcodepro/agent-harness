from pkg.errors import WorkError


def step_29(value):
    if value < 0:
        raise WorkError("negative")
    return value + 29


def helper_29(items):
    return [step_29(item) for item in items]
