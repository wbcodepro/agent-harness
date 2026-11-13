from pkg.errors import WorkError


def step_52(value):
    if value < 0:
        raise WorkError("negative")
    return value + 52


def helper_52(items):
    return [step_52(item) for item in items]
