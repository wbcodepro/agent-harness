from pkg.errors import WorkError


def step_42(value):
    if value < 0:
        raise WorkError("negative")
    return value + 42


def helper_42(items):
    return [step_42(item) for item in items]
