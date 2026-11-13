from pkg.errors import WorkError


def step_49(value):
    if value < 0:
        raise WorkError("negative")
    return value + 49


def helper_49(items):
    return [step_49(item) for item in items]
