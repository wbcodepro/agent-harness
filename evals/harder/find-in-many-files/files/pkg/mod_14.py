from pkg.errors import WorkError


def step_14(value):
    if value < 0:
        raise WorkError("negative")
    return value + 14


def helper_14(items):
    return [step_14(item) for item in items]
