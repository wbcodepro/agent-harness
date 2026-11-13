# merge(base, override)

Returns a new value combining `base` with `override`. Neither argument is modified,
and the result shares no mutable object with either of them.

1. If both are dicts, the result has every key of both. A key present in both is merged
   by these same rules; a key in only one is copied.
2. A key in `override` whose value is `None` removes that key from the result. It is not
   an error if `base` does not have it.
3. A key in `override` that ends with `!` (for example `"db!"`) replaces the key without
   the `!` outright, with no merging, even when both values are dicts. The result never
   contains a key ending in `!`.
4. If both are lists and the first item of `override` is the string `"+"`, the result is
   `base` followed by the rest of `override`. Otherwise the result is `override`.
5. In every other case, including a dict meeting a non-dict, the result is `override`.
