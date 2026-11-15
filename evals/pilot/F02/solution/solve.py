"""Reference solution: a correct transform.py, then run it."""
import subprocess
import sys

SCRIPT = """import csv
import os
from decimal import Decimal


def main():
    totals, counts = {}, {}
    with open("input/orders.csv", newline="", encoding="utf-8") as f:
        for row in csv.DictReader(f):
            if row["status"].strip().lower() != "paid":
                continue
            amount = Decimal(row["amount"].replace(",", ""))
            customer = row["customer"]
            totals[customer] = totals.get(customer, Decimal(0)) + amount
            counts[customer] = counts.get(customer, 0) + 1
    os.makedirs("output", exist_ok=True)
    with open("output/summary.csv", "w", newline="", encoding="utf-8") as out:
        writer = csv.writer(out)
        writer.writerow(["customer", "orders", "total"])
        for customer in sorted(totals, key=lambda c: (-totals[c], c)):
            writer.writerow([customer, counts[customer], f"{totals[customer]:.2f}"])


main()
"""

with open("transform.py", "w", encoding="utf-8") as f:
    f.write(SCRIPT)
subprocess.run([sys.executable, "transform.py"], check=True)
