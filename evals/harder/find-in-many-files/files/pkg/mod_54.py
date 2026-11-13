from pkg.errors import WorkError


def step_54(value):
    if value < 0:
        raise WorkError("negative")
    return value + 54


def helper_54(items):
    return [step_54(item) for item in items]
