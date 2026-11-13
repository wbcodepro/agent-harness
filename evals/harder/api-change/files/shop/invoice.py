from shop.pricing import total


def render(items, tax):
    amount = total(items, tax)
    return f"Total due: {amount:.2f}"


def split(items, tax, people):
    return round(total(items, tax) / people, 2)
