import os


def main():
    totals = {}
    counts = {}
    with open("input/orders.csv") as f:
        lines = f.read().splitlines()
    for line in lines[1:]:
        fields = line.split(",")
        customer = fields[1]
        amount = float(fields[2])
        status = fields[4]
        if status != "paid":
            continue
        totals[customer] = totals.get(customer, 0) + amount
        counts[customer] = counts.get(customer, 0) + 1
    os.makedirs("output", exist_ok=True)
    with open("output/summary.csv", "w") as out:
        out.write("customer,orders,total\n")
        for customer in sorted(totals, key=lambda c: totals[c]):
            out.write(f"{customer},{counts[customer]},{totals[customer]}\n")


main()
