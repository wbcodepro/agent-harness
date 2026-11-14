"""Command line: python3 -m ledgerkit <command> [arguments]

    statement  CUSTOMER YYYY-MM
    average    CUSTOMER YYYY-MM
    proration  PRICE PERIOD_FIRST PERIOD_LAST USED_FIRST USED_LAST
    leave      EMPLOYEE YYYY-MM
    occupancy  ROOM YYYY-MM
    aging      TODAY
    active     DAY
    quarter    YEAR QUARTER
    tax
    by         FIELD        (region, product, channel, method, category,
                             salesperson, currency, warehouse)
"""
import importlib
import sys

from ledgerkit.core import store
from ledgerkit.core.dates import parse_date, parse_month
from ledgerkit.core.errors import BadInput, LedgerError
from ledgerkit.core.money import parse_cents
from ledgerkit.reports import (aging_report, average_report, leave_report, occupancy_report,
                               proration_report, quarter_summary, statement, subscriptions_report,
                               tax_summary)
from ledgerkit.services import customers

BY_FIELDS = ("region", "product", "channel", "method", "category", "salesperson", "currency", "warehouse")


def run(command, args):
    if command == "statement":
        customer = customers.by_id(store.load("customers"), args[0])
        year, month = parse_month(args[1])
        return statement.render(statement.build(store.load("entries"), customer["id"], year, month),
                                customer["name"])
    if command == "average":
        customer = customers.by_id(store.load("customers"), args[0])
        year, month = parse_month(args[1])
        return average_report.render(average_report.build(store.load("entries"), customer["id"], year, month))
    if command == "proration":
        days = [parse_date(text) for text in args[1:5]]
        return proration_report.render(proration_report.build(parse_cents(args[0]), *days))
    if command == "leave":
        year, month = parse_month(args[1])
        return leave_report.render(leave_report.build(store.load("leaves"), args[0], year, month))
    if command == "occupancy":
        year, month = parse_month(args[1])
        return occupancy_report.render(occupancy_report.build(store.load("bookings"), args[0], year, month))
    if command == "aging":
        return aging_report.render(aging_report.build(store.load("invoices"), parse_date(args[0])))
    if command == "active":
        return subscriptions_report.render(subscriptions_report.build(store.load("subscriptions"), parse_date(args[0])))
    if command == "quarter":
        return quarter_summary.render(quarter_summary.build(store.load("entries"), int(args[0]), int(args[1])))
    if command == "tax":
        return tax_summary.render(tax_summary.build(store.load("entries")))
    if command == "by":
        if args[0] not in BY_FIELDS:
            raise BadInput("no report by %r" % (args[0],))
        module = importlib.import_module("ledgerkit.reports.by_" + args[0])
        return module.render(module.build(store.load("entries")))
    raise BadInput("unknown command %r" % (command,))


def main(argv):
    if not argv:
        print(__doc__.strip())
        return 2
    try:
        print(run(argv[0], argv[1:]))
    except IndexError:
        print("missing argument; run without arguments for help", file=sys.stderr)
        return 2
    except (LedgerError, ValueError) as error:
        print("error: %s" % error, file=sys.stderr)
        return 1
    return 0
