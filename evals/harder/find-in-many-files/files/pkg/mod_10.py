from pkg.errors import WorkError


def step_10(value):
    if value < 0:
        raise WorkError("negative")
    return value + 10


def helper_10(items):
    return [step_10(item) for item in items]
