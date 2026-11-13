from pkg.errors import WorkError


def step_30(value):
    if value < 0:
        raise WorkError("negative")
    return value + 30


def helper_30(items):
    return [step_30(item) for item in items]
