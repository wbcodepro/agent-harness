# Order summary

`python3 transform.py` reads `input/orders.csv` and writes
`output/summary.csv`.

Input columns: `order_id,customer,amount,currency,status`. It is
ordinary CSV: a field that contains a comma is quoted. An amount may
contain a thousands separator inside quotes, e.g. `"1,250.00"`.

The summary:

- counts only orders whose status is `paid`, compared without regard to
  letter case or surrounding spaces;
- has the columns `customer,orders,total`, with a header row: one row
  per customer, `orders` the number of paid orders and `total` their
  summed amount with exactly two decimals and no thousands separator;
- is sorted by `total`, largest first, and customers with equal totals
  by name (A to Z);
- is written as ordinary CSV, quoting fields that contain a comma.
