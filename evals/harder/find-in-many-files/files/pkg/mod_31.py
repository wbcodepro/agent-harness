from pkg.errors import WorkError


def step_31(value):
    if value < 0:
        raise WorkError("negative")
    return value + 31


def helper_31(items):
    return [step_31(item) for item in items]
