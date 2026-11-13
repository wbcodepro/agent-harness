from pkg.errors import WorkError


def step_51(value):
    if value < 0:
        raise WorkError("negative")
    return value + 51


def helper_51(items):
    return [step_51(item) for item in items]
