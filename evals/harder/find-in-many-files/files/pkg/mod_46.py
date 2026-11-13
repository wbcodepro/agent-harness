from pkg.errors import WorkError


def step_46(value):
    if value < 0:
        raise WorkError("negative")
    return value + 46


def helper_46(items):
    return [step_46(item) for item in items]
