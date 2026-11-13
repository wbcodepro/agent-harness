from pkg.errors import WorkError


def step_34(value):
    if value < 0:
        raise WorkError("negative")
    return value + 34


def helper_34(items):
    return [step_34(item) for item in items]
