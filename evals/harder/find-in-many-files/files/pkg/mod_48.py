from pkg.errors import WorkError


def step_48(value):
    if value < 0:
        raise WorkError("negative")
    return value + 48


def helper_48(items):
    return [step_48(item) for item in items]
