"""Reading orders files.

An orders file is a sequence of blocks:

    order 1001
    customer Acme Ltd
    item widget 2
    item usb hub 1
    end

Blank lines and lines starting with "#" are ignored.
"""


class OrdersFileError(ValueError):
    """The file is not a valid orders file."""


def _parse_line(number, line):
    """Split one line into (key, value)."""
    key, _, rest = line.partition(" ")
    rest = rest.strip()
    if key == "item":
        name, _, quantity = rest.rpartition(" ")
        if not name or not quantity.isdigit():
            raise OrdersFileError("line %d: expected 'item NAME QUANTITY'" % number)
        return key, (name.strip(), int(quantity))
    if key in ("order", "customer"):
        if not rest:
            raise OrdersFileError("line %d: '%s' needs a value" % (number, key))
        return key, rest
    raise OrdersFileError("line %d: unknown keyword '%s'" % (number, key))


def _collapse(fields):
    """Turn the collected {key: [values]} of one block into a record."""
    return {
        key: values[0] if len(values) == 1 else values
        for key, values in fields.items()
    }


def _finish(number, fields):
    for key in ("order", "customer", "item"):
        if key not in fields:
            raise OrdersFileError("block ending at line %d has no '%s'" % (number, key))
    record = _collapse(fields)
    record["order"] = int(record["order"])
    return record


def load_orders(path):
    """Return the orders in the file at `path`, in file order.

    Each order is a dict:

        {"order": 1001, "customer": "Acme Ltd",
         "item": [("widget", 2), ("usb hub", 1)]}

    "item" holds one (name, quantity) tuple per item line of the order.
    """
    orders = []
    fields = None
    with open(path, encoding="utf-8") as handle:
        for number, raw in enumerate(handle, 1):
            line = raw.strip()
            if not line or line.startswith("#"):
                continue
            if line == "end":
                if fields is None:
                    raise OrdersFileError("line %d: 'end' without a block" % number)
                orders.append(_finish(number, fields))
                fields = None
                continue
            key, value = _parse_line(number, line)
            if key == "order":
                if fields is not None:
                    raise OrdersFileError("line %d: previous block not ended" % number)
                fields = {}
            elif fields is None:
                raise OrdersFileError("line %d: '%s' outside a block" % (number, key))
            fields.setdefault(key, []).append(value)
    if fields is not None:
        raise OrdersFileError("last block not ended")
    return orders
