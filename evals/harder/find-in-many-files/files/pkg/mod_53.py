from pkg.errors import WorkError


def step_53(value):
    if value < 0:
        raise WorkError("negative")
    return value + 53


def helper_53(items):
    return [step_53(item) for item in items]
