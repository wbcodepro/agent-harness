# Contacts files

A contacts file is a JSON object with two keys, `"version"` and `"records"`
(a list of record objects). A directory of contacts holds any number of such
files; only files directly in the directory whose name ends in `.json`
belong to it.

## Version 1 (what the code reads and writes today)

    {"version": 1,
     "records": [{"id": 7, "name": "Ada Lovelace", "mail": "ada@example.org",
                  "phone": "555-0101"}]}

A record has `id` (an integer), `name` (a string with at least one
non-blank character) and `mail` (a string). `phone` and any other keys are
optional.

## Version 2 (to be implemented)

    {"version": 2,
     "records": [{"id": 7, "given_name": "Ada", "family_name": "Lovelace",
                  "email": "ada@example.org", "phone": "555-0101"}]}

A record has `id` (an integer), `given_name` (a string with at least one
non-blank character), `family_name` (a string, possibly empty) and `email`
(a string). `phone` and any other keys are optional.

### Converting a version 1 record

- `mail` becomes `email`, value unchanged.
- `name` is split into words at whitespace. The last word is the
  `family_name`; the words before it, joined by single spaces, are the
  `given_name`. A name of one word is a `given_name` with the empty string
  as `family_name`.
- `id` and every other key (`phone`, or anything else) are carried over
  unchanged. A key that was absent stays absent.

### Problems

A record that does not have the shape its file's version requires (it is
not an object, a required key is missing, or a value has the wrong type) is
malformed. A file is unreadable when it is not valid JSON, is not an object,
has a version other than 1 or 2, or has a `records` value that is not a
list. Neither is ever skipped silently and neither raises an exception;
each is reported as a problem, a dict

    {"file": "vendors.json", "index": 1, "reason": "name is missing"}

where `file` is the file's base name, `index` is the position of the
malformed record in that file's `records` list (0-based), or `None` for an
unreadable file, and `reason` is a non-empty string for a human reader.

### Functions

`store.load_file(path)` returns `(records, problems)`. The records are
always in version 2 shape, whichever version the file has, in file order;
well-formed records of a file are returned even when others in it are
malformed. An unreadable file gives no records and one problem.

`store.load_dir(directory)` returns `(records, problems)` for all files of
the directory, read in sorted file name order.

`store.save_file(path, records)` writes version 2 records as a version 2
file.

`migrate.migrate_dir(directory)` brings a directory up to version 2 and
returns a dict with four keys. `"migrated"`, `"current"` and `"failed"` are
sorted lists of file base names, and every file of the directory is in
exactly one of them; `"problems"` is the list of all problems found.

- A version 1 file without problems is rewritten in place as version 2 and
  listed under `"migrated"`.
- A version 2 file without problems is not written to and is listed under
  `"current"`.
- A file with any problem is listed under `"failed"` and is left exactly as
  it was, byte for byte, so that nothing is lost and someone can repair it.

Running `migrate_dir` again on the same directory migrates nothing more.

`search.find_by_email(records, address)` compares against `email`.
`search.label(record)` returns `"Family, Given"`, or just the given name
when the family name is empty. `search.by_name(records)` sorts by family
name, then given name, both ignoring case, then id.
