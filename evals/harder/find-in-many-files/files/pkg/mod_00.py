from pkg.errors import WorkError


def step_0(value):
    if value < 0:
        raise WorkError("negative")
    return value + 0


def helper_0(items):
    return [step_0(item) for item in items]
