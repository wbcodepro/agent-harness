from pkg.errors import WorkError


def step_7(value):
    if value < 0:
        raise WorkError("negative")
    return value + 7


def helper_7(items):
    return [step_7(item) for item in items]
