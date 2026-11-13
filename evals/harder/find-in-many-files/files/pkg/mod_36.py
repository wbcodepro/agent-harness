from pkg.errors import WorkError


def step_36(value):
    if value < 0:
        raise WorkError("negative")
    return value + 36


def helper_36(items):
    return [step_36(item) for item in items]
