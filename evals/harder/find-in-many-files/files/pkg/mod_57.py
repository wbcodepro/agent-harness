from pkg.errors import WorkError


def step_57(value):
    if value < 0:
        raise WorkError("negative")
    return value + 57


def helper_57(items):
    return [step_57(item) for item in items]
