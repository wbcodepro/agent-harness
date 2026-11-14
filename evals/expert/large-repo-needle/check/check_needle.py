"""Checks that periods behave as docs/CONVENTIONS.md says, everywhere."""
import os
import subprocess
import sys
from datetime import date

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from ledgerkit.core import dates, periods
from ledgerkit.reports import (average_report, leave_report, occupancy_report, proration_report,
                               quarter_summary, statement)
from ledgerkit.services import (aging, averages, discounts, leave, ledger, occupancy, proration,
                                refunds, shipping, subscriptions)

failures = []


def check(label, got, want):
    if got != want:
        failures.append("%s: got %r, want %r" % (label, got, want))


def attempt(label, function):
    try:
        function()
    except Exception as error:  # a crash is a failure of that area, not of the whole check
        failures.append("%s: raised %s: %s" % (label, type(error).__name__, error))


D = date


def e(customer, day, cents, description="Item"):
    return {"customer": customer, "date": day, "description": description, "amount_cents": cents}


ENTRIES = [
    e("C-1", "2023-11-30", 700), e("C-1", "2023-12-01", 100), e("C-1", "2023-12-31", 250),
    e("C-1", "2024-01-01", 400), e("C-1", "2024-01-31", 1000), e("C-1", "2024-02-01", 2000),
    e("C-1", "2024-02-15", -300, "Refund"), e("C-1", "2024-02-29", 4000), e("C-1", "2024-03-01", 8000),
    e("C-1", "2024-03-31", 1600), e("C-1", "2024-04-01", 3200),
    e("C-2", "2024-02-29", 999), e("C-2", "2024-02-10", 1),
]


def statements():
    feb = statement.build(ENTRIES, "C-1", 2024, 2)
    check("statement Feb 2024 dates", [x["date"] for x in feb["entries"]],
          ["2024-02-01", "2024-02-15", "2024-02-29"])
    check("statement Feb 2024 total", feb["total_cents"], 5700)
    dec = statement.build(ENTRIES, "C-1", 2023, 12)
    check("statement Dec 2023 dates", [x["date"] for x in dec["entries"]], ["2023-12-01", "2023-12-31"])
    check("statement Dec 2023 total", dec["total_cents"], 350)
    mar = statement.build(ENTRIES, "C-1", 2024, 3)
    check("statement Mar 2024 total", mar["total_cents"], 9600)
    text = statement.render(statement.build(ENTRIES, "C-2", 2024, 2), "Name")
    check("statement text", text,
          "Statement for C-2 (Name), 2024-02-01 to 2024-02-29\n"
          "2024-02-10  Item                        0.01\n"
          "2024-02-29  Item                        9.99\n"
          "            Total                      10.00")


def command_line():
    run = subprocess.run([sys.executable, "-m", "ledgerkit", "statement", "C-104", "2024-02"],
                         capture_output=True, text=True, timeout=30)
    check("statement command", run.stdout,
          "Statement for C-104 (Dana Whitfield), 2024-02-01 to 2024-02-29\n"
          "2024-02-03  Hosting                    12.00\n"
          "2024-02-14  Support plan               30.00\n"
          "2024-02-21  Cabling                    19.50\n"
          "2024-02-29  Backup storage             25.00\n"
          "            Total                      86.50\n")


def period_helpers():
    check("span of one day", periods.span(D(2024, 5, 5), D(2024, 5, 5)), 1)
    check("span of January", periods.span(D(2024, 1, 1), D(2024, 1, 31)), 31)
    check("contains last day", periods.contains(D(2024, 1, 1), D(2024, 1, 31), D(2024, 1, 31)), True)
    check("contains first day", periods.contains(D(2024, 1, 1), D(2024, 1, 31), D(2024, 1, 1)), True)
    check("contains day after", periods.contains(D(2024, 1, 1), D(2024, 1, 31), D(2024, 2, 1)), False)
    check("contains day before", periods.contains(D(2024, 1, 1), D(2024, 1, 31), D(2023, 12, 31)), False)
    check("one-day period contains its day", periods.contains(D(2024, 5, 5), D(2024, 5, 5), D(2024, 5, 5)), True)
    check("overlap of one shared day",
          periods.overlap_days(D(2024, 1, 1), D(2024, 1, 10), D(2024, 1, 10), D(2024, 1, 20)), 1)
    check("overlap of disjoint periods",
          periods.overlap_days(D(2024, 1, 1), D(2024, 1, 10), D(2024, 1, 11), D(2024, 1, 20)), 0)
    check("overlap inside", periods.overlap_days(D(2024, 1, 1), D(2024, 1, 31), D(2024, 1, 5), D(2024, 1, 9)), 5)
    check("days_between same day", dates.days_between(D(2024, 1, 1), D(2024, 1, 1)), 0)
    check("days_between a month", dates.days_between(D(2024, 1, 1), D(2024, 1, 31)), 30)


def ledger_and_friends():
    got = ledger.entries_in_period(ENTRIES, D(2024, 2, 29), D(2024, 2, 29))
    check("ledger one-day period", [x["amount_cents"] for x in got], [4000, 999])
    check("ledger total over the year end", ledger.total_in_period(ENTRIES, D(2023, 12, 31), D(2024, 1, 1)), 650)
    check("refunds in period", refunds.refunded_in_period(ENTRIES, D(2024, 2, 1), D(2024, 2, 15)), 300)
    q1 = quarter_summary.build(ENTRIES, 2024, 1)
    check("quarter net", q1["net_cents"], 400 + 1000 + 2000 - 300 + 4000 + 8000 + 1600 + 999 + 1)
    subs = [{"customer": "C-1", "plan": "p", "from": "2024-01-15", "to": "2024-02-29", "monthly_cents": 500},
            {"customer": "C-2", "plan": "p", "from": "2024-03-01", "to": "2024-03-01", "monthly_cents": 70}]
    check("subscription on its last day", subscriptions.monthly_recurring_cents(subs, D(2024, 2, 29)), 500)
    check("subscription the day after", subscriptions.monthly_recurring_cents(subs, D(2024, 3, 1)), 70)
    check("subscription one-day", len(subscriptions.active_on(subs, D(2024, 3, 2))), 0)
    codes = [{"code": "LEAP", "percent": 5, "from": "2024-02-29", "to": "2024-02-29"},
             {"code": "WINTER", "percent": 5, "from": "2023-12-01", "to": "2024-02-29"}]
    check("discount codes on last day", discounts.valid_codes(codes, D(2024, 2, 29)), ["LEAP", "WINTER"])
    check("discount codes after", discounts.valid_codes(codes, D(2024, 3, 1)), [])


def prorations():
    feb = (D(2024, 2, 1), D(2024, 2, 29))
    check("proration days", proration.used_days(feb[0], feb[1], D(2024, 2, 10), D(2024, 2, 29)), (20, 29))
    check("proration 20 of 29 days", proration.prorated_cents(2900, feb[0], feb[1], D(2024, 2, 10), D(2024, 2, 29)), 2000)
    check("proration one day", proration.prorated_cents(2900, feb[0], feb[1], D(2024, 2, 10), D(2024, 2, 10)), 100)
    check("proration whole period", proration.prorated_cents(2999, feb[0], feb[1], D(2024, 1, 1), D(2024, 12, 31)), 2999)
    check("proration outside", proration.prorated_cents(2900, feb[0], feb[1], D(2024, 3, 1), D(2024, 3, 9)), 0)
    check("proration rounding", proration.prorated_cents(1000, D(2024, 1, 1), D(2024, 1, 31), D(2024, 1, 1), D(2024, 1, 10)), 323)
    report = proration_report.build(3100, D(2024, 1, 1), D(2024, 1, 31), D(2024, 1, 31), D(2024, 2, 5))
    check("proration report", (report["used_days"], report["period_days"], report["owed_cents"]), (1, 31, 100))


LEAVES = [
    {"employee": "E-1", "from": "2024-02-05", "to": "2024-02-09", "kind": "holiday"},
    {"employee": "E-1", "from": "2024-02-19", "to": "2024-02-19", "kind": "sick"},
    {"employee": "E-1", "from": "2024-02-28", "to": "2024-03-04", "kind": "holiday"},
    {"employee": "E-1", "from": "2024-01-20", "to": "2024-02-01", "kind": "parental"},
    {"employee": "E-2", "from": "2024-02-01", "to": "2024-02-29", "kind": "holiday"},
]


def leaves():
    feb = (D(2024, 2, 1), D(2024, 2, 29))
    check("leave days", leave.days_taken(LEAVES, "E-1", *feb), 5 + 1 + 2 + 1)
    check("leave by kind", leave.days_by_kind(LEAVES, "E-1", *feb), [("holiday", 7), ("parental", 1), ("sick", 1)])
    check("leave whole month", leave.days_taken(LEAVES, "E-2", *feb), 29)
    check("leave nobody", leave.days_taken(LEAVES, "E-9", *feb), 0)
    report = leave_report.build(LEAVES, "E-1", 2024, 3)
    check("leave report March", (report["days"], report["by_kind"]), (4, [("holiday", 4)]))


def daily_averages():
    check("per day over January", averages.per_day_cents(3100, D(2024, 1, 1), D(2024, 1, 31)), 100)
    check("per day over one day", averages.per_day_cents(777, D(2024, 1, 1), D(2024, 1, 1)), 777)
    check("per day rounding", averages.per_day_cents(1000, D(2024, 2, 1), D(2024, 2, 29)), 34)
    own = ledger.entries_of(ENTRIES, "C-1")
    check("daily average Feb", averages.daily_average_cents(own, D(2024, 2, 1), D(2024, 2, 29)), 197)
    report = average_report.build(ENTRIES, "C-1", 2024, 2)
    check("average report", (report["total_cents"], report["per_day_cents"]), (5700, 197))


BOOKINGS = [
    {"room": "R01", "from": "2024-03-01", "to": "2024-03-03", "guest": "a"},
    {"room": "R01", "from": "2024-03-10", "to": "2024-03-10", "guest": "b"},
    {"room": "R01", "from": "2024-03-30", "to": "2024-04-02", "guest": "c"},
    {"room": "R02", "from": "2024-03-05", "to": "2024-03-06", "guest": "d"},
]


def rooms():
    march = (D(2024, 3, 1), D(2024, 3, 31))
    check("days occupied R01", occupancy.days_occupied(BOOKINGS, "R01", *march), 6)
    check("days occupied R02", occupancy.days_occupied(BOOKINGS, "R02", *march), 2)
    check("days occupied R03", occupancy.days_occupied(BOOKINGS, "R03", *march), 0)
    check("occupancy percent", occupancy.occupancy_percent(BOOKINGS, "R01", *march), 19)
    check("occupancy percent ten days", occupancy.occupancy_percent(BOOKINGS, "R01", D(2024, 3, 1), D(2024, 3, 10)), 40)
    report = occupancy_report.build(BOOKINGS, "R01", 2024, 4)
    check("occupancy report April", (report["days"], report["percent"]), (2, 7))


def differences():
    invoices = [
        {"id": "INV-0001", "customer": "C-1", "issued": "2024-03-01", "amount_cents": 1000, "paid": False},
        {"id": "INV-0002", "customer": "C-1", "issued": "2024-01-31", "amount_cents": 2000, "paid": False},
        {"id": "INV-0003", "customer": "C-2", "issued": "2024-01-30", "amount_cents": 4000, "paid": False},
        {"id": "INV-0004", "customer": "C-2", "issued": "2023-12-31", "amount_cents": 8000, "paid": False},
        {"id": "INV-0005", "customer": "C-2", "issued": "2024-03-31", "amount_cents": 16000, "paid": False},
    ]
    check("aging buckets", aging.open_by_bucket(invoices, D(2024, 3, 31)),
          {"0-30": 17000, "31-60": 2000, "61-90": 4000, "90+": 8000})
    check("transit same day", shipping.transit_days({"id": "S", "sent": "2024-02-01", "arrived": "2024-02-01", "warehouse": "W"}), 0)
    check("transit over month end", shipping.transit_days({"id": "S", "sent": "2024-02-27", "arrived": "2024-03-02", "warehouse": "W"}), 4)


for name, function in [("statements", statements), ("command line", command_line),
                       ("period helpers", period_helpers), ("ledger", ledger_and_friends),
                       ("proration", prorations), ("leave", leaves), ("averages", daily_averages),
                       ("occupancy", rooms), ("ages and transit", differences)]:
    attempt(name, function)

if failures:
    print("%d failed:" % len(failures))
    for line in failures:
        print("  " + line)
    sys.exit(1)
print("ok")
