def price_order(items, customer, region):
    total = 0
    for item in items:
        total += item["price"] * item["quantity"]
    discount = 0
    if customer.get("member"):
        discount = total * 0.1
    if total > 1000:
        discount += (total - 1000) * 0.05
    discount = round(discount, 2)
    taxable = total - discount
    rates = {"north": 0.2, "south": 0.1}
    tax = round(taxable * rates.get(region, 0.15), 2)
    return round(taxable + tax, 2)
