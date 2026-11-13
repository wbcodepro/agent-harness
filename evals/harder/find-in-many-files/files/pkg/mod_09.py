from pkg.errors import WorkError


def step_9(value):
    if value < 0:
        raise WorkError("negative")
    return value + 9


def helper_9(items):
    return [step_9(item) for item in items]
