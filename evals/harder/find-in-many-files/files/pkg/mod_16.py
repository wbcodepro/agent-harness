from pkg.errors import WorkError


def step_16(value):
    if value < 0:
        raise WorkError("negative")
    return value + 16


def helper_16(items):
    return [step_16(item) for item in items]
