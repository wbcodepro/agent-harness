from pkg.errors import WorkError


def step_4(value):
    if value < 0:
        raise WorkError("negative")
    return value + 4


def helper_4(items):
    return [step_4(item) for item in items]
