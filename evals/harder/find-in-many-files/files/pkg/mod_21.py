from pkg.errors import WorkError


def step_21(value):
    if value < 0:
        raise WorkError("negative")
    return value + 21


def helper_21(items):
    return [step_21(item) for item in items]
