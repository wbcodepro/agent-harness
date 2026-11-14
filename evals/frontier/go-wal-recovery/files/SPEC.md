# wal: a small durable key-value log

Package `wal` (module `wal`, Go standard library only) stores string keys
with byte-slice values. Every change is appended to segment files in one
directory, and `Open` rebuilds the state from those files. This document is
the whole contract and every numbered rule is binding. Where the example at
the end and a rule seem to disagree, the rule wins.

## API

    type Options struct {
        FS          FS    // nil means OSFS{}
        SegmentSize int64 // 0 means DefaultSegmentSize
    }

    func Open(dir string, opts Options) (*Log, error)

    func (l *Log) Put(key string, value []byte) (seq uint64, err error)
    func (l *Log) Delete(key string) (seq uint64, err error)
    func (l *Log) Apply(rec Record) (applied bool, err error)
    func (l *Log) Batch(ops []Op) (last uint64, err error)
    func (l *Log) Get(key string) (value []byte, ok bool)
    func (l *Log) Keys() []string
    func (l *Log) LastSeq() uint64
    func (l *Log) Compact() error
    func (l *Log) Close() error

The interfaces `FS` and `File`, the type `OSFS`, the types `Options`,
`Record`, `Op`, `Kind` and `CorruptError`, the constants and the error values are
declared in `fs.go` and `wal.go`. Keep those declarations exactly as they
are (you may add to the package, but do not change or remove them). A `Log`
is used from one goroutine at a time.

The file system, as `fs.go` describes it: `ReadDir` returns the names of
the files in a directory in no particular order; `Create` creates a file,
or empties an existing one, and opens it for writing; `OpenAppend` opens an
existing file for writing; every `Write` appends at the end of the file;
`Close` releases the file even when it returns an error.

## Files

1. Every access to the file system goes through `Options.FS`. Outside of
   `OSFS` the package does not call any function of `os`, `io/ioutil` or
   `path/filepath` that touches the file system. The path of the file
   `name` in the directory `dir` is `dir + "/" + name`.
2. A *segment* is a file whose name is exactly eight decimal digits
   followed by `.seg`, whose number is at least 1: `00000001.seg`. A
   *leftover* is a file whose name ends in `.tmp`. Every other file is
   *foreign*, for example `notes.txt`, `1.seg`, `00000000.seg`,
   `000000012.seg`, `00000002.SEG` and `00000003.seg.bak`. The package
   never reads, changes or removes a foreign file.
3. A segment starts with a 16-byte header:

       bytes 0-3    the ASCII characters WSEG
       bytes 4-5    version, uint16 little-endian, always 1
       bytes 6-7    flags, uint16 little-endian; bit 0 (value 1) marks a
                    snapshot segment; all other bits are 0
       bytes 8-15   in a snapshot segment: the snapshot sequence number,
                    uint64 little-endian; in any other segment: bytes
                    8-11 hold the segment's link (rules 39 to 41), uint32
                    little-endian, and bytes 12-15 are 0

4. After the header come records, back to back, each laid out as:

       4 bytes   N, the length of the payload, uint32 little-endian
       4 bytes   CRC-32 (IEEE, as hash/crc32.ChecksumIEEE computes it) of
                 the payload, uint32 little-endian
       N bytes   the payload:
                   8 bytes  sequence number, uint64 little-endian
                   1 byte   kind: 1 = put, 2 = delete; in a batch
                            (rules 34 to 38) bit 7 (BatchFlag, 0x80) may
                            be set as well
                   2 bytes  K, the length of the key, uint16 little-endian
                   K bytes  the key
                   N-11-K bytes  the value

   A delete record has an empty value. A record therefore takes
   `R = 8 + N` bytes in the file.
5. A key is at most `MaxKeySize` (65535) bytes and may be empty. A value is
   at most `MaxValueSize` (1 MiB) bytes and may be empty. So
   `11 <= N <= MaxPayload`.

## Writing

6. `Put` appends a put record with sequence number `LastSeq()+1` and
   returns that number. `Delete` does the same with a delete record; it
   appends a record even when the key does not exist. `LastSeq` is the
   sequence number of the last record applied, 0 for a new log.
7. `Apply` appends a record whose sequence number the caller chose (a
   replica replaying its leader). When `rec.Seq <= LastSeq()` the record is
   *stale*: nothing is written, nothing changes, and the result is
   `(false, nil)`. Otherwise the record is appended with `rec.Seq` (gaps
   are fine), it takes effect, `LastSeq` becomes `rec.Seq`, and the result
   is `(true, nil)`. Applying the same records a second time therefore
   changes nothing.
8. Before anything is written the arguments are checked, in this order: a
   key longer than `MaxKeySize` gives `ErrKeyTooLarge`; a value longer than
   `MaxValueSize` gives `ErrValueTooLarge`; in `Apply` only, a kind other
   than `KindPut` and `KindDelete`, or a `KindDelete` record with a
   non-empty value, gives `ErrInvalidRecord`. These errors are returned
   as the values themselves, they change nothing (no file system call, no
   change of state) and the log stays usable. They come before the
   staleness test of rule 7.
9. The log appends to its *active segment*. Let `size` be the active
   segment's current length in bytes, header included. A record of `R`
   bytes goes into the active segment when `size` is 16 (the segment holds
   no record yet) or when `size + R <= SegmentSize`. Otherwise the log
   closes the active segment's file, creates the segment with the next
   number as described in rule 10, makes it the active segment and appends
   the record there. So a record bigger than `SegmentSize` gets a segment
   of its own.
10. A segment is created with `FS.Create`, then its header (with the link
    of rule 40 when it is not a snapshot) is written,
    then `Sync` is called. A record is appended with `Write` and then
    `Sync` (a batch: rule 36). `Put`, `Delete` and `Apply` return success only after that
    `Sync` has returned nil, and only then does the change become visible
    through `Get`, `Keys` and `LastSeq`.
11. `Get` returns a copy of the stored value (changing it does not change
    the log), and `Put` and `Apply` store a copy of the caller's slice
    (changing the slice afterwards does not change the log).
12. `Get` of a key that exists returns `(value, true)` where `value` has
    the stored length; a key put with an empty value exists. `Get` of a
    key that does not exist or was deleted returns `(nil, false)`. `Keys`
    returns the existing keys in ascending byte order (the order of
    `sort.Strings`).
13. When the file system returns an error `e` during `Put`, `Delete` or
    `Apply` (a rotation included), the call returns an error for which
    `errors.Is(err, e)` holds, the in-memory state (`Get`, `Keys`,
    `LastSeq`) is exactly what it was before the call, and the log is
    *poisoned*: from then on `Put`, `Delete`, `Apply` and `Compact` return
    `ErrPoisoned` without calling the file system, before the checks of
    rule 8. `Get`, `Keys`, `LastSeq` and `Close` keep working.
14. `Close` closes the open file. It returns nil, or, when that file's
    `Close` fails with `e`, an error for which `errors.Is(err, e)` holds;
    either way the log is closed afterwards. After `Close`: `Put`,
    `Delete`, `Apply` and `Compact` return `ErrClosed` (also on a poisoned
    log, and before the checks of rule 8); `Get`, `Keys` and `LastSeq` keep
    answering from memory; another `Close` returns nil.
15. Between calls, a `Log` that is open and not poisoned holds exactly one
    open `File`: its active segment. A poisoned `Log` holds at most one.
    After `Close`, and whenever `Open` returns an error, no `File` obtained
    from the `FS` is left open. This holds on every error path, including
    errors in the middle of a rotation or of `Compact`.

## Opening

16. `Open` with a negative `SegmentSize` returns `ErrInvalidOptions`
    without calling the file system. Otherwise it calls
    `FS.MkdirAll(dir)`, lists the directory and considers the segments in
    ascending order of their numbers.
17. Let `S` be the highest-numbered segment that is at least 16 bytes long
    and has a valid header (rule 19) with the snapshot flag set. The
    segments with a lower number than `S` are *obsolete*: whatever their
    content, they never cause an error. `S` and the segments above it are
    *live*. Without such an `S` every segment is live.
18. The live segments are checked and replayed in ascending order. A
    problem found is either *torn* (rule 22) or *corrupt*. A corrupt
    segment makes `Open` fail with a `*CorruptError` whose `Segment` is
    the segment's file name (without the directory) and whose `Offset` is
    the byte offset of the problem in that file; `errors.Is(err,
    ErrCorrupt)` holds for it. When there are several problems, the first
    in replay order (lowest segment number, then lowest offset) is the one
    reported.
19. A header is valid when the magic is `WSEG`, the version is 1, no flag
    bit other than bit 0 is set, and a segment without the snapshot flag
    has 0 in bytes 12-15. A live segment with an invalid header
    is corrupt at offset 0.
20. A live segment shorter than 16 bytes is a *torn creation* if it is the
    highest-numbered segment; `Open` then creates it afresh (rule 10) as an
    empty segment that is not a snapshot. A live segment shorter than 16
    bytes in any other position is corrupt at offset 0.
21. Records are read from offset 16 on. For a record starting at offset
    `p` in a file of `z` bytes, these tests are made in this order:
    - `z - p == 0`: the segment ends here.
    - `z - p < 8`: torn.
    - `N < 11` or `N > MaxPayload`: corrupt at `p`.
    - `z - p - 8 < N`: torn.
    - the CRC does not match: torn if the record ends exactly at the end
      of the file (`p + 8 + N == z`), otherwise corrupt at `p`.
    - the kind byte is not 1, 2, 0x81 or 0x82, or `K > N - 11`, or it is
      a delete (2 or 0x82) with a non-empty value: corrupt at `p`.

    Otherwise the record is good (rule 37 adds what batches require) and the next one starts at `p + 8 + N`.
22. A torn record is acceptable only in the highest-numbered segment, and
    only when that segment is not a snapshot: the segment then ends at `p`
    and `Open` cuts the file back to `p` bytes with `FS.Truncate`. A torn
    record in any other segment, or in a snapshot, is corrupt at `p`.
23. In a snapshot segment every record must be a put with kind byte 1
    (bit 7 clear), the sequence numbers
    must be strictly increasing, and none may exceed the header's snapshot
    sequence number. A good record (rule 21) that breaks this is corrupt at
    its offset.
24. Replay starts with no keys and `LastSeq` 0. If there is a snapshot
    `S`, its records are applied and then `LastSeq` becomes the header's
    snapshot sequence number (also when the snapshot holds no records). In
    the other live segments a record whose sequence number is not greater
    than the current `LastSeq` is stale: it is skipped silently and stays
    in the file. This is how duplicate sequence numbers are resolved: the
    first record with a given number wins. Every other record is applied
    and sets `LastSeq`.
25. `Open` is all or nothing with respect to corruption. When it returns a
    `*CorruptError` the directory is byte for byte what it was before the
    call: no leftover or obsolete segment removed, nothing truncated,
    nothing created. Everything is validated before anything is changed.
26. When validation succeeds `Open` removes every leftover and every
    obsolete segment, cuts back a torn record (rule 22) and re-creates a
    torn creation (rule 20). The active segment is the highest-numbered
    segment; but when that is a snapshot, or when there is no segment at
    all, a new segment is created (number: the highest number plus one, or
    1) and becomes the active segment. An existing active segment is
    opened with `FS.OpenAppend`.
27. When the file system returns an error `e` during `Open`, `Open` returns
    an error for which `errors.Is(err, e)` holds (and rule 15 applies).

## Compaction

28. `Compact` replaces the whole log by a snapshot. With `A` the number of
    the active segment it does exactly this, in this order:
    1. Create the leftover `<A+1>.seg.tmp` (the number written with eight
       digits, like a segment name), write a snapshot header whose
       snapshot sequence number is `LastSeq()`, then one put record for
       every existing key, carrying the key's current value and the
       sequence number of the record that last set that key, in ascending
       order of those sequence numbers. Then `Sync`, then `Close` it.
    2. `FS.Rename` it to `<A+1>.seg`.
    3. Close the active segment's file and create segment `<A+2>.seg`
       (rule 10) as the new, empty active segment.
    4. `FS.Remove` every segment with a number of `A` or less.
29. After a successful `Compact` the directory holds exactly the snapshot
    `<A+1>.seg`, the active segment `<A+2>.seg` (16 bytes) and the foreign
    files; `Get`, `Keys` and `LastSeq` answer as before. The sequence
    number that last set a key survives compaction and reopening, so a
    second `Compact` writes it again.
30. A file-system error `e` anywhere in `Compact` is returned (an error for
    which `errors.Is(err, e)` holds), poisons the log (rule 13) and leaves
    the in-memory state as it was. Rule 15 applies.

## Durability

31. The process may die at any moment, also inside `Open`. Data written to
    a file but not yet synced may then be lost, wholly or in part: the
    file is left ending somewhere within its unsynced data, and the bytes
    that remain are the ones written. A completed `Create`, `Rename`,
    `Remove` or `Truncate`, and data that a `Sync` has returned for, are
    never lost.
32. Whatever the moment of death, a later `Open` of the directory must
    succeed and must show every change whose call returned success,
    possibly also the one change whose call was in progress, and nothing
    else. A `Compact` changes nothing visible: keys, values and `LastSeq`
    are the same before, during and after it, and so after a death in the
    middle of it. The steps and orders of rules 10 and 28 are what make
    this true; do not reorder or skip them.
33. The same holds when a call failed (rules 13, 27 and 30) instead of the
    process dying: after `Close`, a later `Open` must succeed and show
    every change whose call returned success, possibly the change of the
    call that failed, and nothing else. After every such `Open` the log
    works normally.

## Batches

34. `Batch` appends the ops as records with the sequence numbers
    `LastSeq()+1`, `LastSeq()+2`, ... in the order given, and returns the
    sequence number of the last of them. In the file every record of the
    batch except the last has bit 7 of its kind byte set (kind byte 0x81
    for a put, 0x82 for a delete); the last record has it clear. A batch
    of one op is therefore written exactly like a `Put` or `Delete`.
35. `Batch` returns `ErrClosed` and `ErrPoisoned` as `Put` does (rules 13
    and 14). Then an empty (or nil) `ops` gives `ErrEmptyBatch`. Then the
    ops are checked one after the other, in order, each with the checks
    of rule 8 in the order given there: key, value, and then the kind,
    which must be `KindPut` or `KindDelete` (a kind with bit 7 set is
    invalid), with an empty value for `KindDelete`. The first error found
    is returned. These errors change nothing, as in rule 8.
36. All records of a batch are appended with a single `Write`, followed by
    one `Sync`, into one segment: for rule 9, `R` is the total size of
    all the batch's records. `Batch` returns success only after that
    `Sync` has returned nil; only then do all its ops become visible,
    together (keys, values, `LastSeq`). On a file system error none of
    them is visible (rule 13). `Batch` stores copies of the values (rule
    11).
37. When Open reads records (rule 21), a record whose kind byte has bit 7
    set opens a batch, or continues the batch that is open; the next
    record with bit 7 clear belongs to that batch and ends it. A record
    with bit 7 clear read while no batch is open is a batch of its own.
    After a record that continues an open batch has passed the tests of
    rule 21, its sequence number must be exactly one more than that of
    the record before it, or it is corrupt at its offset. When the
    segment ends, or a torn record is found, while a batch is open, the
    problem is torn at the offset of that batch's first record instead;
    rule 22 then applies to that offset (in the highest segment the file
    is cut back to where the batch began).
38. During replay (rule 24) a batch takes effect only as a whole, once its
    last record has been read. The batch is stale, and skipped as a whole
    (it stays in the file), when its first record's sequence number is
    not greater than the current `LastSeq`; otherwise all of its records
    are applied in order and `LastSeq` becomes the sequence number of its
    last record. For rules 32 and 33 a `Batch` call is one change: after
    a death or a failure, either all of its ops are visible or none.

## Chaining

39. The *chain value* of a segment is the CRC field (bytes 4-7 of the
    record, as a uint32) of the last record in the segment. For a segment
    without records it is the segment's link, and 0 for a snapshot. Every
    record in the file counts: stale records, records of skipped batches
    and the records of batches all do. For a highest segment whose torn
    end Open cuts off (rules 22 and 37), the chain value is that of what
    remains after the cut.
40. Every segment the package creates that is not a snapshot gets as its
    link the chain value of the live segment with the next lower number,
    or 0 when there is none: on rotation, the active segment it follows;
    in step 3 of `Compact`, the snapshot `<A+1>`; in `Open`, the live
    segment below a re-created torn creation or below a new active
    segment. A new log's first segment therefore has link 0.
41. During `Open`, right after the header test of rule 19 and before its
    records are read, a live segment that is not a snapshot has its link
    checked: it must equal the chain value of the live segment with the
    next lower number, or be 0 when it is the lowest live segment.
    Otherwise the segment is corrupt at offset 8. A torn creation (rule
    20) has no link to check.

## Example

A new log in an empty directory: `Open` creates `00000001.seg`, 16 bytes:

    57 53 45 47 01 00 00 00 00 00 00 00 00 00 00 00

`Put("ab", []byte("xyz"))` returns 1 and appends this 24-byte record
(N = 16):

    10 00 00 00 73 c9 54 38 01 00 00 00 00 00 00 00
    01 02 00 61 62 78 79 7a

`Delete("ab")` then returns 2 and appends a 21-byte record with N = 13.
`Compact` (with `A` = 1) leaves `00000002.seg`, a snapshot holding no
records whose header is

    57 53 45 47 01 00 01 00 02 00 00 00 00 00 00 00

and `00000003.seg`, an empty active segment whose link is 0 because the
snapshot holds no records; `00000001.seg` is removed and `LastSeq` is
still 2. Had the log instead rotated to `00000002.seg` right after the
put, that segment's header would have been

    57 53 45 47 01 00 00 00 73 c9 54 38 00 00 00 00

since 73 c9 54 38 is the CRC field of the last record in `00000001.seg`.
