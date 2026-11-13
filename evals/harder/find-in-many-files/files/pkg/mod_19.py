from pkg.errors import WorkError


def step_19(value):
    if value < 0:
        raise WorkError("negative")
    return value + 19


def helper_19(items):
    return [step_19(item) for item in items]
