# ledgerkit

Bookkeeping reports for a small business. The data lives in `data/*.json`.

    python3 -m ledgerkit                       # list of commands
    python3 -m ledgerkit statement C-104 2024-02
    python3 -m unittest discover -s tests

Layout: `ledgerkit/core` holds shared helpers, `ledgerkit/services` the
business rules, `ledgerkit/reports` one module per report, and
`ledgerkit/validators`, `ledgerkit/models` and `ledgerkit/formatters` one
module per kind of record or output. `docs/CONVENTIONS.md` says how dates,
periods and money are meant to behave everywhere.
