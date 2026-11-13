from pkg.errors import WorkError


def step_2(value):
    if value < 0:
        raise WorkError("negative")
    return value + 2


def helper_2(items):
    return [step_2(item) for item in items]
