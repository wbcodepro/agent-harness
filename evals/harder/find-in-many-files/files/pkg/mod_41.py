from pkg.errors import WorkError


def step_41(value):
    if value < 0:
        raise WorkError("negative")
    return value + 41


def helper_41(items):
    return [step_41(item) for item in items]
