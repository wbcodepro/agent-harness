"""Command line: python3 -m salesreport top FILE [COUNT] | stats FILE"""

import sys

from .lines import order_lines
from .loader import OrdersFileError, load_orders
from .report import render_top
from .stats import render_stats

USAGE = "usage: python3 -m salesreport top FILE [COUNT] | stats FILE"


def main(argv):
    if len(argv) < 2 or argv[0] not in ("top", "stats"):
        print(USAGE, file=sys.stderr)
        return 2
    try:
        orders = load_orders(argv[1])
    except (OSError, OrdersFileError) as error:
        print("error: %s" % error, file=sys.stderr)
        return 1
    if argv[0] == "stats":
        print(render_stats(orders))
        return 0
    count = int(argv[2]) if len(argv) > 2 else 3
    lines = order_lines(orders)
    print(render_top(lines, count))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
