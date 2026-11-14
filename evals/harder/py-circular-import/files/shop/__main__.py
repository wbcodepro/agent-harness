from shop.orders import Order

order = Order("lan")
order.add("pen", 3)
order.add("ink", 1)
print(order.receipt())
