from pkg.errors import WorkError


def step_45(value):
    if value < 0:
        raise WorkError("negative")
    return value + 45


def helper_45(items):
    return [step_45(item) for item in items]
