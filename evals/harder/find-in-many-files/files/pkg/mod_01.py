from pkg.errors import WorkError


def step_1(value):
    if value < 0:
        raise WorkError("negative")
    return value + 1


def helper_1(items):
    return [step_1(item) for item in items]
