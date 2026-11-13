from pkg.errors import WorkError


def step_59(value):
    if value < 0:
        raise WorkError("negative")
    return value + 59


def helper_59(items):
    return [step_59(item) for item in items]
