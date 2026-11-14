# Conventions

These hold for every module. Code that does not follow them is wrong.

## Dates

Dates are `datetime.date` objects inside the code and ISO strings
(`YYYY-MM-DD`) in the data files and on the command line.

## Periods

A period is written as its first day and its last day, and both days belong
to it. This is true for months and quarters, for bookings ("from" and "to"
are the first and the last day the guest has the room), for leave (the
first and the last day off), for subscriptions and for discount codes.

- A day is in the period when it is not before the first day and not after
  the last day. The last day itself is in the period.
- The length of a period counts both ends: 1 January to 31 January is 31
  days, and a period whose first and last day are the same day is 1 day
  long.
- Two periods that share only one day overlap by 1 day.

A *difference* between two dates is another thing: the invoice age and the
transit time of a shipment are 0 when both dates are the same day.

## Money

Amounts are integer cents. Whenever a division or a percentage does not
come out even, the result is rounded half up to the cent (or, for
percentages of days, to the whole percent).
