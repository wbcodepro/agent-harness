# Ledger engine

`ledger.py` provides one class, `Ledger`.

- `Ledger(currencies)` creates an empty ledger.
- `ledger.apply(op)` processes one operation and returns a result dict.

`apply` never raises, whatever JSON value it is given. The caller may change
`op` after the call, and may change the dict that `apply` returned; neither
may affect the ledger or anything it returns later. `apply` does not modify
`op`.

Only the Python standard library may be used.

## 1. Currencies

`currencies` maps a currency code to `{"exponent": E, "rounding": MODE}`.
`E` is an int from 0 to 4: the number of digits after the decimal point. The
smallest amount of a currency, 10^-E, is its *minor unit*. All arithmetic is
exact; binary floating point must not be used anywhere.

`MODE` says how an exact value is rounded to a whole number of minor units:

| mode        | rule                                             | 2.5 | -2.5 | 2.1 | -2.1 | 3.5 |
|-------------|--------------------------------------------------|-----|------|-----|------|-----|
| `half_even` | to nearest; a tie goes to the even neighbour     | 2   | -2   | 2   | -2   | 4   |
| `half_up`   | to nearest; a tie goes away from zero            | 3   | -3   | 2   | -2   | 4   |
| `half_down` | to nearest; a tie goes toward zero               | 2   | -2   | 2   | -2   | 3   |
| `up`        | away from zero                                   | 3   | -3   | 3   | -3   | 4   |
| `down`      | toward zero                                      | 2   | -2   | 2   | -2   | 3   |
| `ceiling`   | toward positive infinity                         | 3   | -2   | 3   | -2   | 4   |
| `floor`     | toward negative infinity                         | 2   | -3   | 2   | -3   | 3   |

## 2. Text formats

R1. *Amount text* is: an optional `-`, one or more of the ASCII digits
`0`-`9`, and optionally a `.` followed by one or more ASCII digits. Nothing
else is allowed: no `+`, no exponent, no spaces, no line break, no digits
outside ASCII, no empty string. Leading zeros are fine (`007.5`), and `-0`
and `-0.00` are amount text meaning zero.

R2. *Rate text* is amount text with at most 6 digits after the point.

R3. A *date* is exactly ten characters `YYYY-MM-DD`, ASCII digits and two
hyphens, naming a real day of the proleptic Gregorian calendar with a year
from 0001 to 9999. `2024-02-30`, `2023-02-29`, `2024-2-5` and `20240205` are
not dates. A *period* is exactly seven characters `YYYY-MM` with a month
from 01 to 12 and a year from 0001 to 9999. A date *is in* the period that
equals its first seven characters.

R4. An *account id* is 1 to 16 characters, each an ASCII letter, an ASCII
digit, `_` or `-`. Ids are case-sensitive.

R5. Every amount in a result is formatted from a whole number of minor units
of the relevant currency: an optional `-`, the integer part without leading
zeros (a single `0` if it is zero), and, when the exponent is greater than 0,
a `.` and exactly `E` digits. Zero is never written with a `-`. Examples at
exponent 2: `0.00`, `-0.05`, `1234.50`; at exponent 0: `0`, `-17`.

## 3. Operations

An operation is a dict with a string field `"op"` and the fields of that
operation. Every field is a string, except `"parts"` (see `split`). Fields
marked *optional* may be left out; `null` is not a string, so a field whose
value is `null` has the wrong type.

| op          | required fields                         | optional      |
|-------------|-----------------------------------------|---------------|
| `open`      | `account`, `currency`                   | `min_balance` |
| `deposit`   | `key`, `date`, `account`, `amount`      |               |
| `withdraw`  | `key`, `date`, `account`, `amount`      |               |
| `transfer`  | `key`, `date`, `from`, `to`, `amount`   | `rate`        |
| `split`     | `key`, `date`, `from`, `amount`, `parts`|               |
| `accrue`    | `key`, `date`, `account`, `rate`        |               |
| `refund`    | `key`, `date`, `of`, `amount`           |               |
| `reverse`   | `key`, `date`, `of`                     |               |
| `freeze`    | `account`                               |               |
| `unfreeze`  | `account`                               |               |
| `close`     | `period`                                |               |
| `balance`   | `account`                               | `as_of`       |
| `statement` | `account`, `period`                     |               |

`parts` is a list of dicts, each with exactly the two fields `"account"`
(string) and `"weight"` (an int; `true` and `false` are not ints).

The seven operations that have a `key` are the *posting operations*.

### Results

R6. A failed operation returns exactly `{"ok": False, "error": CODE}` and
changes nothing at all: no balance, no transaction number, no remembered key.
Section 6 lists the codes and says which one is reported.

R7. A successful posting operation creates a *transaction*. Transactions are
numbered from 1 in the order they are created; the id is `T` followed by the
number (`T1`, `T2`, ... `T10`, ...). Only successful posting operations
consume a number.

R8. A transaction has a date and a list of *entries*. An entry adds a signed
amount to one account on the transaction's date. An entry whose amount is
zero is not recorded and not reported, so a transaction can have fewer
entries than expected, or none.

R9. A successful posting operation returns

    {"ok": True, "tx": ID, "date": DATE, "entries": [{"account": A, "amount": TEXT}, ...]}

with the entries in the order stated for the operation and each amount
formatted in the currency of its account. A `refund` result has one more
field, `"remaining"` (R27).

R10. The *balance* of an account is the sum of all its entries, whatever
their dates. An account starts with no entries.

### open

R11. Creates the account `account` in `currency`. `min_balance` is amount
text in that currency and defaults to zero; it is the lowest balance that
`withdraw`, `transfer` and `split` may leave on the account. Returns
`{"ok": True}`.

### deposit, withdraw

R12. `deposit` records one entry of `+amount` on `account`. `withdraw`
records one entry of `-amount`.

### transfer

R13. Moves `amount`, in the currency of `from`, out of `from` and into `to`.
When the two accounts have the same currency `to` receives the same amount
and `rate` must be absent. When the currencies differ `rate` must be present,
and `to` receives `amount x rate`, computed exactly and then rounded to a
whole number of minor units of the currency of `to`, using that currency's
rounding mode. Entries: `from` first, then `to`.

### split

R14. Takes `amount` out of `from` and divides it among the accounts in
`parts`, which must all have the currency of `from`. Let `W` be the sum of
the weights and `M` the amount in minor units. Part `i` first receives
`floor(M x w_i / W)` minor units. The minor units that are then left over
are handed out one at a time, at most one per part, to the parts with the
largest fractional part of `M x w_i / W`; among equal fractional parts the
larger weight comes first, and among equal weights the part that comes
earlier in `parts`. A part whose fractional part is zero never receives a
leftover unit. Fractions are compared exactly.

Entries: `from` first (`-amount`), then the parts in the order of `parts`.

### accrue

R15. Records one entry of `balance x rate` on `account`, where `balance` is
the account's balance (R10) before the operation, the product is exact, and
the result is rounded to minor units with the account currency's rounding
mode. `rate` may be negative or zero, and so may the balance. `accrue`
ignores `min_balance`.

### refund

R16. `of` names an earlier `transfer` or `split`, the *original*. `amount`
is in the currency of the original's `from` account and says how much of the
original amount is given back now. The sum of all refunds of one original
can never exceed the original's amount. The refund is a new transaction
with its own date; the original is not altered.

R17. Refund of a transfer. `from` gets `+amount`. For `to`, let `S` be the
original amount, `D` what `to` originally received, and `C` the total
refunded from this original so far, *including* this refund, all in minor
units. After this refund the total taken back from `to` must be
`D x C / S`, computed exactly and rounded with the rounding mode of the
currency of `to`. This refund takes from `to` the difference between that
total and what earlier refunds of the same original already took. (For a
transfer within one currency this is simply `amount`.) Entries: `from`,
then `to`.

R18. Refund of a split. `from` gets `+amount`. Each part has a *remainder*:
what it originally received minus what earlier refunds of the same original
took back from it. `amount` is divided among the parts by the method of R14
with the remainders as weights, except that among equal fractional parts
the larger *remainder* comes first, and among equal remainders the part
that comes earlier in the original's `parts`. Each part gets minus its
share. Entries: `from`, then the parts in the original's order.

### reverse

R19. `of` names an earlier `deposit`, `withdraw`, `accrue`, `transfer` or
`split`. A transaction can be reversed once.

- Reversing a `deposit`, `withdraw` or `accrue` records the original's
  entries with the opposite sign.
- Reversing a `transfer` or `split` gives back everything that has not been
  refunded yet: it has exactly the entries that a `refund` of the whole
  unrefunded amount would have.

A reversed transaction can be neither refunded nor reversed again.

R20. A `reverse` is never rejected because of a closed period. When its
`date` is in a closed period, the transaction is dated the first day of the
first month that is not closed, and that date is the one stored and
reported in `"date"`. (R33 still compares the `date` as given.)

### freeze, unfreeze

R21. `freeze` marks the account frozen, `unfreeze` clears the mark. Both
return `{"ok": True}`, also when the account already is in that state. A
frozen account cannot be the `account` of a `withdraw` or the `from` of a
`transfer` or `split`. Nothing else is affected: deposits, incoming
transfers, shares of a split, accruals, refunds and reversals all work on a
frozen account, also when they take money from it.

### close

R22. Closes `period` and every period before it. `period` must be later
than the latest period closed so far. Returns

    {"ok": True, "closed": PERIOD, "balances": {ACCOUNT: TEXT, ...}}

with one item for every account that exists, giving the sum of its entries
dated on or before the last day of `period`.

R23. After a close, no posting operation other than `reverse` may be dated
in a closed period.

### balance

R24. Returns `{"ok": True, "currency": CODE, "balance": TEXT}`. With
`as_of` (a date) the balance is the sum of the entries dated on or before
that day; without it, of all entries.

### statement

R25. Returns

    {"ok": True, "opening": TEXT, "lines": [...], "closing": TEXT}

`opening` is the sum of the account's entries dated before the first day of
`period`. `lines` has one item per entry of the account dated in `period`,
ordered by date and, within a date, by transaction number (so `T9` comes
before `T10`):

    {"date": DATE, "tx": ID, "amount": TEXT, "balance": TEXT}

where `balance` is `opening` plus the amounts of this and all preceding
lines. `closing` is the last line's balance, or `opening` when there are no
lines.

## 4. Idempotency keys

R26. `key` is a non-empty string. When a posting operation succeeds, the
ledger remembers its key together with the operation exactly as it was
given. Keys are shared by all posting operations.

When a later operation carries a remembered key:

- if it is equal to the remembered operation (the same fields with the same
  values, compared as given: `"10.5"` and `"10.50"` are different values),
  it is a *replay*. Nothing changes; the result is the result the first
  call returned, with one more field `"replayed": True`. This holds even
  when the operation would fail if it were new, for example because its
  period has been closed since.
- otherwise it fails with `KEY_CONFLICT`.

Keys of failed operations are not remembered (R6).

## 5. Refund bookkeeping

R27. `"remaining"` in a refund result is the original's amount minus all
refunds of it so far, including this one, formatted in the currency of the
original's `from` account.

R28. Refunds and reversals ignore `min_balance`: they may leave any account
with any balance.

## 6. Errors

When an operation fails, exactly one code is reported: of all the codes
whose condition holds, **the one that comes first in this table**. A
condition that cannot be evaluated because an earlier-listed condition
holds (the account does not exist, the amount is not amount text, ...)
simply does not hold.

| #  | code               | condition |
|----|--------------------|-----------|
| 1  | `BAD_OP`           | R29 |
| 2  | `BAD_DATE`         | `date` or `as_of` is not a date, or `period` is not a period (R3) |
| 3  | `BAD_AMOUNT`       | `amount` or `min_balance` is not amount text; `rate` is not rate text; the `rate` of a `transfer` is zero or negative |
| 4  | `BAD_ACCOUNT_ID`   | `open`: `account` is not an account id (R4) |
| 5  | `KEY_CONFLICT`     | R26 (a replay is also recognised at this point, R30) |
| 6  | `UNKNOWN_CURRENCY` | `open`: `currency` is not a configured code |
| 7  | `ACCOUNT_EXISTS`   | `open`: the account already exists |
| 8  | `UNKNOWN_ACCOUNT`  | any other op: `account`, `from`, `to` or the account of any part does not exist |
| 9  | `UNKNOWN_TX`       | `of` is not the id of a transaction |
| 10 | `NOT_REFUNDABLE`   | `refund`: the original is not a `transfer` or `split` |
| 11 | `NOT_REVERSIBLE`   | `reverse`: the original is a `refund` or a `reverse` |
| 12 | `ALREADY_REVERSED` | `refund` or `reverse`: the original has been reversed |
| 13 | `PRECISION`        | R31 |
| 14 | `NOT_POSITIVE`     | the `amount` of a `deposit`, `withdraw`, `transfer`, `split` or `refund` is zero or negative |
| 15 | `BAD_LIMIT`        | `open`: `min_balance` is greater than zero |
| 16 | `SAME_ACCOUNT`     | `transfer`: `from` and `to` are the same account |
| 17 | `BAD_PARTS`        | R32 |
| 18 | `CURRENCY_MISMATCH`| `split`: a part's account has another currency than `from` |
| 19 | `RATE_REQUIRED`    | `transfer`: the currencies differ and there is no `rate` |
| 20 | `RATE_FORBIDDEN`   | `transfer`: the currencies are the same and there is a `rate` |
| 21 | `DATE_ORDER`       | R33 |
| 22 | `PERIOD_CLOSED`    | a posting operation other than `reverse` has a `date` in a closed period |
| 23 | `ALREADY_CLOSED`   | `close`: `period` is not later than the latest closed period |
| 24 | `FROZEN`           | R21 |
| 25 | `OVER_REFUND`      | `refund`: `amount` is greater than the original's amount minus the refunds of it so far |
| 26 | `NOTHING_TO_REVERSE` | R34 |
| 27 | `INSUFFICIENT`     | `withdraw`, `transfer`, `split`: the balance (R10) of `account`/`from` minus `amount` is less than its `min_balance` |

R29. `BAD_OP`: `op` is not a dict; it has no `"op"` field, or that field is
not one of the thirteen names (this includes values that are not strings);
a required field is missing; a field is present that the table in section 3
does not list for this operation; a field's value has the wrong type; `key`
is the empty string; an item of `parts` is not a dict with exactly the
fields `account` (string) and `weight` (int, not a bool).

R30. Row 5 is where keys are looked at. An operation that fails with a code
from rows 1 to 4 is reported as such even if its key is remembered. An
operation that gets past row 4 with a remembered key is either a replay or
a `KEY_CONFLICT`; nothing below row 5 is evaluated for it.

R31. `PRECISION`: an amount has more digits after the point than the
exponent of its currency, even if the extra digits are zeros (`1.10` is too
precise for exponent 1). The currency is: for the `min_balance` of `open`,
the account's currency; for `deposit` and `withdraw`, the currency of
`account`; for `transfer` and `split`, of `from`; for `refund`, of the
original's `from`.

R32. `BAD_PARTS`: `parts` is empty; or has more than 20 items; or names the
same account twice; or names the `from` account; or has a negative weight;
or all its weights are zero.

R33. `DATE_ORDER`: the `date` of a `refund` or `reverse`, as given in the
operation, is earlier than the date of the original.

R34. `NOTHING_TO_REVERSE`: `reverse` of a transaction that has no entries
(R8), or of a `transfer` or `split` that has been refunded completely.

## 7. Worked example

Currencies: `USD` exponent 2 `half_even`, `JPY` exponent 0 `half_up`.
Accounts `a`, `b`, `c`, `d` in USD and `j` in JPY, all with the default
`min_balance`.

1. `deposit` 100.00 to `a` on 2024-01-10 -> `T1`.
2. `split` of 1.00 from `a` on 2024-01-11 with weights `b` 1, `c` 1, `d` 1
   -> `T2`. `M` = 100, each part first gets 33, one unit is left, all
   fractional parts and weights are equal, so the earliest part gets it.
   Entries: `a` -1.00, `b` 0.34, `c` 0.33, `d` 0.33.
3. `refund` of `T2`, amount 0.50, on 2024-01-12 -> `T3`. Remainders are 34,
   33, 33; exact shares are 17, 16.5, 16.5; one unit is left over, `b` has
   fractional part zero and gets none, `c` comes before `d`. Entries: `a`
   0.50, `b` -0.17, `c` -0.17, `d` -0.16. `remaining` is `0.50`.
4. `transfer` of 10.00 from `a` to `j` on 2024-01-12 with rate `0.333` ->
   `T4`. 10.00 x 0.333 = 3.33 JPY, rounded to 3. Entries: `a` -10.00, `j` 3.
5. `refund` of `T4`, amount 5.00 -> `T5`. `C` = 500, 3 x 500 / 1000 = 1.5,
   rounded half up to 2. Entries: `a` 5.00, `j` -2. `remaining` `5.00`.
6. `refund` of `T4`, amount 5.00 -> `T6`. `C` = 1000, total to take back is
   3, of which 2 already taken. Entries: `a` 5.00, `j` -1. `remaining`
   `0.00`.
7. `reverse` of `T4` fails with `NOTHING_TO_REVERSE`. No transaction number
   is used, so the next transaction is `T7`.
