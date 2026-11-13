from pkg.errors import WorkError


def step_56(value):
    if value < 0:
        raise WorkError("negative")
    return value + 56


def helper_56(items):
    return [step_56(item) for item in items]
