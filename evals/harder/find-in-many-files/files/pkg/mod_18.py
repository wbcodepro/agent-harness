from pkg.errors import WorkError


def step_18(value):
    if value < 0:
        raise WorkError("negative")
    return value + 18


def helper_18(items):
    return [step_18(item) for item in items]
