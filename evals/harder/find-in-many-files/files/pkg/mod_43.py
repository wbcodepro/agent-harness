from pkg.errors import WorkError


def step_43(value):
    if value < 0:
        raise WorkError("negative")
    return value + 43


def helper_43(items):
    return [step_43(item) for item in items]
