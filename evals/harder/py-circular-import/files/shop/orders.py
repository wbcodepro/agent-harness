from shop.pricing import line_total, PRICES


class Order:
    def __init__(self, customer):
        self.customer = customer
        self.lines = []

    def add(self, item, quantity):
        if item not in PRICES:
            raise KeyError(item)
        self.lines.append((item, quantity))

    def total(self):
        return sum(line_total(self, item, quantity) for item, quantity in self.lines)

    def receipt(self):
        rows = [f"{item} x{quantity}" for item, quantity in self.lines]
        return "\n".join(rows + [f"total {self.total():.2f}"])


VIP = {"lan"}
