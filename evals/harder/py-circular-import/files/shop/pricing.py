from shop.orders import VIP

PRICES = {"pen": 2.0, "ink": 9.5}


def line_total(order, item, quantity):
    amount = PRICES[item] * quantity
    if order.customer in VIP:
        amount *= 0.9
    return amount
