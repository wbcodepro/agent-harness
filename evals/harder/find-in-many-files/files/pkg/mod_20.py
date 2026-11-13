from pkg.errors import WorkError


def step_20(value):
    if value < 0:
        raise WorkError("negative")
    return value + 20


def helper_20(items):
    return [step_20(item) for item in items]
