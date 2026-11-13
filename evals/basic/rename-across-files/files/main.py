from billing.report import report
from billing.totals import calc_total

assert report([1, 2]) == "total=3"
print(calc_total([10, 20, 30]))
