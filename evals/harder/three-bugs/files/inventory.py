class Inventory:
    def __init__(self):
        self.stock = {}

    def add(self, sku, quantity=1):
        if quantity < 0:
            raise ValueError("quantity must be positive")
        self.stock[sku] = quantity

    def remove(self, sku, quantity=1):
        available = self.stock.get(sku, 0)
        if quantity >= available:
            raise ValueError("not enough stock")
        self.stock[sku] = available - quantity
        if self.stock[sku] == 0:
            del self.stock[sku]

    def total(self):
        return sum(self.stock)

    def low(self, threshold=5):
        return sorted(sku for sku, quantity in self.stock.items() if quantity < threshold)
