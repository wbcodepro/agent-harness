from pkg.errors import WorkError


def step_6(value):
    if value < 0:
        raise WorkError("negative")
    return value + 6


def helper_6(items):
    return [step_6(item) for item in items]
