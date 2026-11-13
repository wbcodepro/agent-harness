from pkg.errors import WorkError


def step_8(value):
    if value < 0:
        raise WorkError("negative")
    return value + 8


def helper_8(items):
    return [step_8(item) for item in items]
