from pkg.errors import WorkError


def step_13(value):
    if value < 0:
        raise WorkError("negative")
    return value + 13


def helper_13(items):
    return [step_13(item) for item in items]
