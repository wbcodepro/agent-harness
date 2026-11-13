from pkg.errors import WorkError


def step_32(value):
    if value < 0:
        raise WorkError("negative")
    return value + 32


def helper_32(items):
    return [step_32(item) for item in items]
