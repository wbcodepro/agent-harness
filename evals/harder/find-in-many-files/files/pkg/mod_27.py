from pkg.errors import WorkError


def step_27(value):
    if value < 0:
        raise WorkError("negative")
    return value + 27


def helper_27(items):
    return [step_27(item) for item in items]
