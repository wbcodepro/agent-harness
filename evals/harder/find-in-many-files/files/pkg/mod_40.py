from pkg.errors import WorkError


def step_40(value):
    if value < 0:
        raise WorkError("negative")
    return value + 40


def helper_40(items):
    return [step_40(item) for item in items]
