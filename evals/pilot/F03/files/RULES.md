# Filing rules for inbox/

Apply these to every file directly inside `inbox/`. Names are compared
as plain byte strings (ASCII order: `-` sorts before `.`, capitals
before lower case). Extensions are matched without regard to case.

1. Duplicates first. Files with identical contents form a group. In
   each group only the file whose name sorts first is filed by rules
   2-4; every other file of the group goes to
   `archive/duplicates/<original name>`.
2. A `.pdf` whose name starts with `invoice-YYYY-MM` (four digits, a
   dash, two digits) goes to `archive/invoices/YYYY/MM/<original name>`.
3. A `.jpg` or `.jpeg` goes to `archive/photos/<name in lower case>`.
4. A `.txt` goes to `archive/notes/<original name>`.
5. Everything else stays in `inbox/` unchanged.

Move files; never change their contents, and never delete a file.
