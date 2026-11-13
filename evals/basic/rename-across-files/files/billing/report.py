from billing.totals import calc_total


def report(items):
    return f"total={calc_total(items)}"
