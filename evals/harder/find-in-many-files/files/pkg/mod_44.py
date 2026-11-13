from pkg.errors import WorkError


def step_44(value):
    if value < 0:
        raise WorkError("negative")
    return value + 44


def helper_44(items):
    return [step_44(item) for item in items]
