from pkg.errors import WorkError


def step_22(value):
    if value < 0:
        raise WorkError("negative")
    return value + 22


def helper_22(items):
    return [step_22(item) for item in items]
