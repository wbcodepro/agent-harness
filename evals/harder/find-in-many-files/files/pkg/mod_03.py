from pkg.errors import WorkError


def step_3(value):
    if value < 0:
        raise WorkError("negative")
    return value + 3


def helper_3(items):
    return [step_3(item) for item in items]
