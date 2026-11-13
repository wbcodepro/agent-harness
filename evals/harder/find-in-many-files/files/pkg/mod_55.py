from pkg.errors import WorkError


def step_55(value):
    if value < 0:
        raise WorkError("negative")
    return value + 55


def helper_55(items):
    return [step_55(item) for item in items]
