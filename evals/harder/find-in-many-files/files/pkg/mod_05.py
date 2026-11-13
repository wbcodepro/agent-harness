from pkg.errors import WorkError


def step_5(value):
    if value < 0:
        raise WorkError("negative")
    return value + 5


def helper_5(items):
    return [step_5(item) for item in items]
