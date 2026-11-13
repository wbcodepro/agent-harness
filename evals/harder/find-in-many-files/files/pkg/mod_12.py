from pkg.errors import WorkError


def step_12(value):
    if value < 0:
        raise WorkError("negative")
    return value + 12


def helper_12(items):
    return [step_12(item) for item in items]
