from pkg.errors import WorkError


def step_28(value):
    if value < 0:
        raise WorkError("negative")
    return value + 28


def helper_28(items):
    return [step_28(item) for item in items]
