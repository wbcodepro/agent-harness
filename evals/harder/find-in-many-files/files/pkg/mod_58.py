from pkg.errors import WorkError


def step_58(value):
    if value < 0:
        raise WorkError("negative")
    return value + 58


def helper_58(items):
    return [step_58(item) for item in items]
