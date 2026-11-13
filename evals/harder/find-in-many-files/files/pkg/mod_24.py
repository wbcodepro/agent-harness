from pkg.errors import WorkError


def step_24(value):
    if value < 0:
        raise WorkError("negative")
    return value + 24


def helper_24(items):
    return [step_24(item) for item in items]
