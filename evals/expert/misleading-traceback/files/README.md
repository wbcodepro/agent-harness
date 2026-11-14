# salesreport

Small reports over an orders file.

## Orders files

An orders file is a sequence of blocks. Blank lines and lines starting with
`#` are ignored.

    order 1001
    customer Acme Ltd
    item widget 2
    item usb hub 1
    end

Every block has one `order` line (the order number), one `customer` line and
one or more `item` lines. An item line is `item NAME QUANTITY`; the quantity
is the last word and the name, which may contain spaces, is everything
before it.

## Commands

    python3 -m salesreport top FILE [COUNT]

prints the COUNT (default 3) products with the most units sold over all
orders in the file: most units first, products with equal units in
alphabetical order.

    python3 -m salesreport stats FILE

prints the number of orders, the number of distinct customers, the number of
item lines over all orders, the average number of item lines per order, and
the order with the most item lines (the earliest one in the file on a tie).

## Tests

    python3 -m unittest discover -s tests -q
