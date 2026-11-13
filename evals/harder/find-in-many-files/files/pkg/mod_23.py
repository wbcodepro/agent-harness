from pkg.errors import WorkError


def step_23(value):
    if value < 0:
        raise WorkError("negative")
    return value + 23


def helper_23(items):
    return [step_23(item) for item in items]
