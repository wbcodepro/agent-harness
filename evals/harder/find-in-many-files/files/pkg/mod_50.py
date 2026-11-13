from pkg.errors import WorkError


def step_50(value):
    if value < 0:
        raise WorkError("negative")
    return value + 50


def helper_50(items):
    return [step_50(item) for item in items]
