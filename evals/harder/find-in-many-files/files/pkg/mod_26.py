from pkg.errors import WorkError


def step_26(value):
    if value < 0:
        raise WorkError("negative")
    return value + 26


def helper_26(items):
    return [step_26(item) for item in items]
