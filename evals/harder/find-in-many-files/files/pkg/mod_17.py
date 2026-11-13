from pkg.errors import WorkError


def step_17(value):
    if value < 0:
        raise WorkError("negative")
    return value + 17


def helper_17(items):
    return [step_17(item) for item in items]
