# Access decisions

`decide(request, rules)` in `access.py` returns a tuple `(effect, rule_id)`:
the effect is `"allow"` or `"deny"`, and `rule_id` is the id of the rule that
decided, or `None` when no rule did. Neither argument is modified.

## Inputs

`request` is a dict with `"user"` (str), `"groups"` (list of str, possibly
empty), `"action"` (str), `"path"` (str) and `"now"` (int, seconds).

`rules` is a list of dicts. Each has `"id"` (str, unique), `"effect"`,
`"who"` and `"path"` (a pattern), and may have `"actions"` (list of str),
`"expires"` (int or None) and `"final"` (bool, default False).

## Validation

Before any decision is made, every rule is validated, in list order. The
first invalid rule raises `ValueError` with a message that contains that
rule's id. This happens even if the rule is expired or could never apply to
the request. A rule is invalid when

- its effect is not exactly `"allow"` or `"deny"`, or
- its `who` is not `"*"`, `"user:NAME"` or `"group:NAME"` with a non-empty
  NAME, or
- its path pattern has a `**` segment anywhere but in last position.

## Paths

A path and a pattern are both split on `/` and empty segments are dropped,
so `/a//b/` and `a/b` are the same two segments, and `/` has no segments at
all. Segments are compared exactly; case matters.

In a pattern, a segment that is exactly `*` matches any one segment. A last
segment that is exactly `**` matches all remaining segments of the path,
however many, including none: `/docs/**` matches `/docs` itself. Any other
segment is literal and must be equal; `a*` is literal, not a wildcard.
Without a final `**` the pattern and the path must have the same number of
segments.

## When a rule applies

A rule applies to a request when all four of these hold.

1. Who: `"*"` fits everyone, `"user:NAME"` fits when the request's user is
   NAME, `"group:NAME"` fits when NAME is one of the request's groups. Names
   are compared exactly.
2. Path: the rule's pattern matches the request's path.
3. Action: a rule without an `"actions"` key covers every action. Otherwise
   the request's action must be in the list, ignoring letter case; the entry
   `"*"` covers every action. An empty list therefore covers nothing.
4. Time: a rule whose `"expires"` is an int has expired once `now` is equal
   to or greater than it, and an expired rule never applies. A missing
   `"expires"` and `None` both mean the rule never expires.

## Choosing among the rules that apply

If no rule applies the result is `("deny", None)`.

Otherwise one rule wins and the result is its effect and its id. The
criteria below are used in this order; a later one only matters between
rules that the earlier ones could not separate.

1. Final: if at least one applicable rule has `"final"` true, every
   applicable rule that is not final is out of the running.
2. Literal segments: the rule whose pattern has more literal segments
   (segments that are neither `*` nor `**`) wins.
3. Open end: a pattern without a final `**` beats one with it.
4. Who: a `user:` rule beats a `group:` rule, which beats a `"*"` rule.
5. Effect: deny beats allow.
6. Position: the rule that comes later in the list wins.
