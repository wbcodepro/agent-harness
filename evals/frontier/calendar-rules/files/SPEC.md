# Business calendar

`bizcal.py` provides the functions of section 9 for the four regions defined
here. Everything they need is in this file. Only the Python standard library
may be used, and no time-zone database (`zoneinfo`, `pytz`, `tzdata`, the
`TZ` environment variable): every offset comes from section 7.

The rules are numbered 1 to 41. Where two rules could both decide a
question, the one that states its precedence wins; otherwise every rule
applies.

## 1. Notation

1. A *day* is a string of exactly ten characters `YYYY-MM-DD`: ASCII digits
   and two hyphens, naming a real date of the proleptic Gregorian calendar
   with a year from 1900 to 2199. A *timestamp* is a string of exactly
   sixteen characters `YYYY-MM-DDTHH:MM`: a day, a capital `T`, an hour
   from `00` to `23`, a colon and a minute from `00` to `59`. Nothing else is
   a day or a timestamp. In particular `2021-3-01`, `20210301`,
   `2021-03-01 ` (trailing space), `2021-02-29`, `1899-12-31`,
   `2021-03-01T24:00`, `2021-03-01T9:30`, `2021-03-01 09:30` and
   `2021-03-01T09:30:00` are not.

2. Weekdays are written `mon tue wed thu fri sat sun`. A week runs from
   Monday to Sunday.

3. "from 2015" means the years 2015 and later; "through 2019" means 2019
   and earlier.

## 2. Regions and weekends

4. There are four regions. Each has weekend days and a daily cutoff
   (rule 39):

   | region | weekend days                                                      | cutoff |
   |--------|-------------------------------------------------------------------|--------|
   | `NORD` | sat, sun                                                          | 17:00  |
   | `GULF` | fri, sat on days through 2021-12-31; sat, sun on days from 2022-01-01 | 14:00  |
   | `AUST` | sat, sun                                                          | 16:00  |
   | `ISLE` | sun                                                               | 16:30  |

   Whether a day is a *weekend day* depends on that day alone: in `GULF`,
   Friday 2021-12-31 is a weekend day, and so are Saturday 2022-01-01 and
   Sunday 2022-01-02, while Friday 2022-01-07 is not.

## 3. Holidays

5. Each row of a holiday table is one holiday per year, in the years that
   its "years" column allows (empty: every year). The "years" column refers
   to the year of the nominal date. The *nominal date* in year `Y` is given
   by the "date" column:

   - `MM-DD`: that date in year `Y`. `02-29` exists only in leap years; in
     other years that holiday does not occur at all.
   - `easter+N` / `easter-N`: N days after / before Easter Sunday of year
     `Y` (rule 6).
   - `2nd mon of jun`, `4th thu of nov`, `1st wed of aug`: the n-th such
     weekday of that month of year `Y`.
   - `fri on or after 06-19`: the first such weekday on or after that date
     of year `Y`.
   - `day after ROW k`: the day after the nominal date of row k of the same
     table in year `Y`.

   ### NORD

   | row | name             | date                  | years     | shift   |
   |-----|------------------|-----------------------|-----------|---------|
   | 1   | New Year's Day   | 01-01                 |           | nearest |
   | 2   | Second New Year  | 01-02                 |           | forward |
   | 3   | Good Friday      | easter-2              |           | none    |
   | 4   | Easter Monday    | easter+1              |           | none    |
   | 5   | Labour Day       | 05-01                 |           | forward |
   | 6   | Ascension Day    | easter+39             |           | none    |
   | 7   | Founders' Day    | 2nd mon of jun        | from 2015 | none    |
   | 8   | Midsummer Eve    | fri on or after 06-19 |           | none    |
   | 9   | Christmas Day    | 12-25                 |           | forward |
   | 10  | Boxing Day       | 12-26                 |           | forward |
   | 11  | New Year's Eve   | 12-31                 | from 2010 | backward|

   ### GULF

   | row | name                | date  | years     | shift   |
   |-----|---------------------|-------|-----------|---------|
   | 1   | New Year's Day      | 01-01 |           | nearest |
   | 2   | Commemoration Day   | 11-30 | from 2015 | forward |
   | 3   | National Day        | 12-02 |           | forward |
   | 4   | National Day Two    | 12-03 |           | forward |

   ### AUST

   | row | name                 | date           | years | shift   |
   |-----|----------------------|----------------|-------|---------|
   | 1   | New Year's Day       | 01-01          |       | forward |
   | 2   | Federation Day       | 01-26          |       | forward |
   | 3   | Good Friday          | easter-2       |       | none    |
   | 4   | Easter Saturday      | easter-1       |       | none    |
   | 5   | Easter Monday        | easter+1       |       | none    |
   | 6   | Remembrance Day      | 04-25          |       | forward |
   | 7   | Sovereign's Birthday | 2nd mon of jun |       | none    |
   | 8   | Christmas Day        | 12-25          |       | forward |
   | 9   | Proclamation Day     | 12-26          |       | forward |

   ### ISLE

   | row | name             | date            | years        | shift    |
   |-----|------------------|-----------------|--------------|----------|
   | 1   | New Year's Day   | 01-01           |              | forward  |
   | 2   | Leap Day         | 02-29           |              | nearest  |
   | 3   | Good Friday      | easter-2        |              | none     |
   | 4   | Regatta Day      | 1st wed of aug  | through 2019 | none     |
   | 5   | Regatta Day      | 1st mon of aug  | from 2020    | none     |
   | 6   | Harvest Day      | 4th thu of nov  |              | none     |
   | 7   | Harvest Friday   | day after ROW 6 |              | none     |
   | 8   | Christmas Eve    | 12-24           |              | backward |
   | 9   | Christmas Day    | 12-25           |              | forward  |
   | 10  | St Stephen's Day | 12-26           |              | forward  |

6. Easter Sunday of year `Y` is month `month`, day `day` computed as
   follows (`/` is integer division, `%` the remainder):

       a = Y % 19
       b = Y / 100          c = Y % 100
       d = b / 4            e = b % 4
       f = (b + 8) / 25
       g = (b - f + 1) / 3
       h = (19*a + b - d - g + 15) % 30
       i = c / 4            k = c % 4
       l = (32 + 2*e + 2*i - h - k) % 7
       m = (a + 11*h + 22*l) / 451
       month = (h + l - 7*m + 114) / 31
       day   = (h + l - 7*m + 114) % 31 + 1

## 4. Observed dates

The day on which a holiday is actually taken is its *observed date*. The
observed dates of a region are worked out separately for each year `Y` of
nominal dates (the *pass-year*), in two passes, using only that region's
weekend days (rule 4) and that pass-year's holidays.

7. First pass: every holiday of the pass-year whose shift is `none` is
   observed on its nominal date, whatever kind of day that is. If that is a
   weekend day it is still a holiday on that date.

8. Second pass, step a: the other holidays of the pass-year are taken one
   at a time in the order of their rows. Each starts at its nominal date. If
   that is a weekend day it moves:
   - `forward`: to the first later day that is not a weekend day;
   - `backward`: to the last earlier day that is not a weekend day;
   - `nearest`: to the closest day that is not a weekend day; if the closest
     such earlier day and the closest such later day are equally far away,
     to the later one.

9. Second pass, step b: then, as long as the day reached is a weekend day,
   or is already the observed date of another holiday of the same pass-year
   (any holiday of the first pass, or an earlier row of the second pass),
   the holiday moves on by one day: one day earlier for a `backward`
   holiday, one day later for every other. The day where this stops is its
   observed date. Step b applies even when step a did not move the holiday.

10. An observed date may lie in another calendar year than its nominal date
    (New Year's Day moved back into December). It belongs to the calendar
    year it lies in. Rule 9 only compares holidays of the same pass-year.

11. Special days (section 6) and bridge days (section 5) play no part in
    rules 7 to 10.

Examples. `NORD`, pass-year 2021: 12-25 is a Saturday and 12-26 a Sunday;
Christmas Day is observed on Monday 12-27 and Boxing Day on Tuesday 12-28.
Pass-year 2022: 12-25 is a Sunday; Christmas Day is observed on Monday
12-26, which is then taken, so Boxing Day is observed on Tuesday 12-27.
Pass-year 2022: New Year's Day (a Saturday) is observed on Friday
2021-12-31, and Second New Year (a Sunday) on Monday 2022-01-03.

## 5. Bridge days

12. `NORD` has bridge days on days from 2018-01-01 and `ISLE` on days from
    2020-01-01; the other regions have none, and neither has any on earlier
    days. A day D of a region that has bridge days on D is a *bridge day*
    if all of the following hold:
    - D is not a weekend day and not an observed date of any holiday;
    - one of its two neighbours (the day before D, the day after D) is a
      weekend day;
    - the other neighbour is an observed date of a holiday (of any
      pass-year) and is not a weekend day.

    A day between two holidays, or between two weekend days, is therefore
    not a bridge day. Special days (section 6) play no part in deciding
    whether a day is a bridge day.

13. Bridge days are not holidays: `holidays()` does not list them.

Example. In `NORD`, Ascension Day 2021 is Thursday 2021-05-13, so Friday
2021-05-14 is a bridge day. Christmas Day 2018 is observed on Tuesday
2018-12-25, so Monday 2018-12-24 is a bridge day.

## 6. Special days and business days

14. Special days:

    | region | special closures                                | special working days                 |
    |--------|-------------------------------------------------|--------------------------------------|
    | `NORD` | 2012-06-05, 2022-09-19                          | 2016-12-31, 2020-05-01, 2021-05-14   |
    | `GULF` | 2020-03-26, 2022-05-14, 2022-05-15, 2022-05-16  | 2021-12-31, 2023-12-02               |
    | `AUST` | 2022-09-22                                      | 2010-12-26                           |
    | `ISLE` | 2019-01-21, 2021-12-19                          | 2021-12-19, 2022-12-26, 2024-02-25, 2025-11-29 |

15. Whether a day is a *business day* of a region is decided by the first
    of these lines that applies to it:

    - B1. a special working day of the region is a business day;
    - B2. a special closure of the region is not a business day;
    - B3. an observed date of a holiday of the region is not;
    - B4. a weekend day of the region is not;
    - B5. a bridge day of the region is not;
    - B6. every other day is a business day.

## 7. Clock offsets

16. The *local reading* of an instant in a region is the UTC reading of the
    instant plus the region's offset in force at that instant. Offsets:

    | region | standard | summer | summer time begins                         | summer time ends                            |
    |--------|----------|--------|--------------------------------------------|---------------------------------------------|
    | `NORD` through 1995 | +01:00 | +02:00 | last sun of mar, 01:00 UTC  | last sun of sep, 01:00 UTC                  |
    | `NORD` from 1996    | +01:00 | +02:00 | last sun of mar, 01:00 UTC  | last sun of oct, 01:00 UTC                  |
    | `GULF` | see rule 19 | none |                                          |                                             |
    | `AUST` | +10:00   | +11:00 | 1st sun of oct, 02:00 local standard time  | 1st sun of apr, 03:00 local summer time     |
    | `ISLE` through 2006 | -03:30 | -02:30 | 1st sun of apr, 02:00 local standard time | last sun of oct, 02:00 local summer time |
    | `ISLE` 2007 to 2024 | -03:30 | -02:30 | 2nd sun of mar, 02:00 local standard time | 1st sun of nov, 02:00 local summer time  |
    | `ISLE` from 2025    | see rule 19 | none |                                    |                                          |

    For `NORD` and `ISLE` the year that chooses the line is the year of the
    day on which the transition happens.

17. Each "begins" and "ends" entry names one instant in every year. "01:00
    UTC" is the instant whose UTC reading is 01:00 on that day. "02:00
    local standard time" is the instant whose reading with the standard
    offset is 02:00 on that day; "03:00 local summer time" is the instant
    whose reading with the summer offset is 03:00 on that day.

18. The offset in force at an instant t is decided by the latest transition
    instant at or before t: the summer offset if that transition is a
    "begins", the standard offset if it is an "ends". A transition takes
    effect exactly at its instant. (In `AUST` summer time therefore runs
    from October across the new year to April.)

19. `GULF` has no summer time. Its offset is +04:00 at instants before
    2016-06-30T20:00 UTC and +03:00 from that instant on. `ISLE` has its
    last transition at the instant summer time ends in 2024 (2024-11-03,
    02:00 local summer time, by the table); from that instant on its offset
    is -03:00 at every instant, and rule 18 no longer applies to it.

20. When the offset increases, the local clock jumps forward and the local
    readings in between do not exist; when it decreases, the local clock
    goes back and the local readings in between occur twice. Example for
    `NORD` in 2021: summer time begins at 2021-03-28T01:00 UTC, the local
    clock goes from 02:00 straight to 03:00, and the readings
    2021-03-28T02:00 to 2021-03-28T02:59 do not exist. It ends at
    2021-10-31T01:00 UTC, the local clock goes from 03:00 back to 02:00, and
    the readings 2021-10-31T02:00 to 2021-10-31T02:59 occur twice.

## 8. Recurrence rules

21. A recurrence rule is a dict with these keys:

    | key           | value                                                          |
    |---------------|----------------------------------------------------------------|
    | `freq`        | `"daily"`, `"weekly"`, `"monthly"` or `"yearly"` (required)    |
    | `anchor`      | a day (required)                                               |
    | `interval`    | int >= 1, default 1                                            |
    | `by_weekday`  | weekly only: a non-empty list of weekday names                 |
    | `week_start`  | weekly only: a weekday name, default `"mon"`                   |
    | `pick`        | weekly or monthly only: a non-empty list of ints, none of them 0 |
    | `by_monthday` | monthly only: a non-empty list of ints, each 1..31 or -31..-1  |
    | `nth_weekday` | monthly only: `[n, weekday]`, n in 1..5 or -5..-1              |
    | `business_day`| monthly only: an int, not 0                                    |
    | `clamp`       | bool, default false                                            |
    | `until`       | a day                                                          |
    | `count`       | int >= 0                                                       |
    | `except`      | list of days                                                   |
    | `include`     | list of days                                                   |
    | `adjust`      | `"none"` (default), `"following"`, `"preceding"`, `"modified_following"`, `"modified_preceding"`, `"nearest"` or `"skip"` |

    A rule with a key not in this table, without a required key, with a
    value outside what the table allows, with a "weekly only", "monthly
    only" or "weekly or monthly only" key under another `freq`, or with
    more than one of `by_monthday`, `nth_weekday` and `business_day`, is
    invalid.

22. *Candidates.* The rule yields candidate days in increasing order. No
    candidate is earlier than the anchor; a day that the following would
    produce before the anchor is not a candidate.

    - `daily`: the anchor and every `interval`-th day after it.
    - `weekly`: here a week is the seven days that begin on a `week_start`
      weekday (not the Monday-to-Sunday week of rule 2, unless `week_start`
      is `"mon"`). Take the week that contains the anchor and every
      `interval`-th week after it. In each of those weeks the candidates
      are the days whose weekday is in `by_weekday`; without `by_weekday`,
      the days with the anchor's weekday.
    - `monthly`: take the anchor's month and every `interval`-th month after
      it. In each of those months the candidates are:
      - with `by_monthday`: for each value `v`, day `v` of the month if `v`
        is positive; if `v` is negative, the day `|v| - 1` days before the
        last day of the month (`-1` is the last day). A value that names no
        day of that month gives no candidate in that month; but if `clamp`
        is true, such a positive value gives the last day of the month and
        such a negative value the first day.
      - with `nth_weekday` `[n, w]`: the n-th weekday `w` of the month,
        counted from the end of the month when n is negative (`-1` is the
        last). No candidate in a month that has no such day, whatever
        `clamp` says.
      - with `business_day` `n`: the n-th business day of the month in the
        region that `occurrences` is called with, counted from the end when
        n is negative. No candidate in a month with fewer business days.
      - with none of the three: as `by_monthday` with the single value of
        the anchor's day of the month.
    - `yearly`: the anchor's month and day in the anchor's year and in every
      `interval`-th year after it. In a year where that date does not exist
      (29 February), no candidate; but if `clamp` is true the candidate is
      28 February of that year.
    - `pick` (weekly and monthly only): in each week or month taken above,
      put that period's days, found as just described, in increasing order
      without repeats, and keep only the days at the positions that `pick`
      lists: `1` is the first, `2` the second, `-1` the last, `-2` the one
      before it. A position beyond the number of days keeps nothing; a day
      picked twice is kept once. Picking happens before the anchor rule
      above: positions count every day of the period, including days
      earlier than the anchor, and a picked day earlier than the anchor is
      then not a candidate.

    The same day is never a candidate twice.

23. `until`: candidates later than `until` are dropped.

24. `count`: of the candidates left by rule 23, only the first `count` are
    kept. Counting starts at the anchor; it has nothing to do with the
    `start` and `end` of the query.

25. `except`: candidates equal to one of these days are dropped. This
    happens after rule 24, so an excepted candidate still uses up one of
    the `count`.

26. `include`: each of these days is added to the candidates left by rule
    25. Rules 22 to 25 do not apply to an included day: it may be earlier
    than the anchor, later than `until`, beyond the `count` or listed in
    `except`, and it is still added.

27. `adjust`: every candidate left after rule 26 is replaced by the day
    that `adjust()` (rule 37) gives for it with that convention. With
    `"skip"`, a candidate that is not a business day is dropped instead,
    and business days stay as they are.

28. The result is the list of the days produced by rule 27 that lie from
    `start` to `end` inclusive, each day once, in increasing order. Only
    the day after rule 27 matters: a candidate before `start` whose
    adjusted day lies in the range is in the result, and a candidate in the
    range whose adjusted day lies outside is not.

## 9. Functions

29. Every function raises `ValueError` for a region other than the four
    names of rule 4, for an argument that should be a day or a timestamp
    and is not one (rule 1), for a rule that rule 21 makes invalid, and
    where this section says so. Arguments are passed positionally, except
    `prefer` and `also`, which may also be passed by keyword. What a
    function returns or raises when its result would be a day or timestamp
    outside the years 1900 to 2199 is not specified.

30. `is_business_day(region, day)` -> bool: rule 15.

31. `holidays(region, year)` -> list of `(day, name)` tuples: every holiday
    of the region, of any pass-year, whose observed date lies in the
    calendar year `year`, ordered by observed date. Holidays with the same
    observed date are ordered by pass-year, earlier pass-year first, and
    within one pass-year by row. `year` is an int from 1900 to 2199,
    otherwise `ValueError`. Special days do not add anything
    to the list or remove anything from it; a holiday observed on a special
    working day is still listed.

32. `add_business_days(region, day, n)` -> day. For `n > 0`, the n-th
    business day after `day`; for `n < 0`, the |n|-th business day before
    `day`; `day` itself is never counted. For `n == 0`: `day` if it is a
    business day, otherwise the first business day after it.

33. `business_days_between(region, start, end)` -> int. If `start <= end`,
    the number of business days d with `start < d <= end`; otherwise the
    negative of the number of business days d with `end < d <= start`.

34. `nth_business_day(region, year, month, n)` -> day. The n-th business
    day of that month, counted from the end when n is negative (`-1` is the
    last business day of the month). `ValueError` if `n` is 0, if the month
    has fewer than |n| business days, if `year` is not from 1900 to 2199 or
    if `month` is not from 1 to 12.

35. `utc_offset(region, utc)` -> int: the offset in minutes (east of UTC
    positive) in force at the instant whose UTC reading is the timestamp
    `utc`.

36. `to_local(region, utc)` -> timestamp: the local reading of that
    instant.

37. `adjust(region, day, convention)` -> day. A business day is returned
    unchanged under every convention. Otherwise:
    - `"none"`: `day` unchanged;
    - `"following"`: the first business day after `day`;
    - `"preceding"`: the last business day before `day`;
    - `"modified_following"`: the first business day after `day`, unless
      that lies in a later calendar month than `day`; then the last business
      day before `day`;
    - `"modified_preceding"`: the last business day before `day`, unless
      that lies in an earlier calendar month than `day`; then the first
      business day after `day`;
    - `"nearest"`: of the last business day before `day` and the first
      business day after `day`, the one fewer days away from `day`; if both
      are equally far away, the one after `day`.

    Any other convention, including `"skip"`, raises `ValueError`.

38. `to_utc(region, local, prefer="earlier")` -> timestamp: the UTC reading
    of the instant at which the region's local reading is `local`. If that
    reading does not exist (rule 20), `ValueError`. If it occurs twice, the
    earlier of the two instants when `prefer` is `"earlier"` and the later
    one when it is `"later"`. `prefer` has no effect on other readings, but
    any value of `prefer` other than those two raises `ValueError`.

39. `settle(region, utc, days, also=None)` -> day, for an int `days >= 0`
    (otherwise `ValueError`). `also` is `None` or a region name (otherwise
    `ValueError`). Take the local reading of the instant whose UTC reading
    is `utc`. The *trade day* is the date of that reading if it is a
    business day and the time of the reading is earlier than the cutoff in
    force on that date; otherwise it is the first business day after that
    date. The cutoff in force is the region's cutoff (rule 4), except on the
    last business day before the observed date of a holiday named
    `Christmas Day` of the region, where it is 12:00. Trade day and cutoff
    depend on `region` alone. When `also` is `None`, the result is
    `add_business_days(region, trade_day, days)`. When `also` is a region,
    a *joint business day* is a day that is a business day of `region` and
    of `also`, and the result is worked out like `add_business_days` but
    counting joint business days: for `days > 0` the `days`-th joint
    business day after the trade day (the trade day itself is never
    counted), and for `days == 0` the trade day if it is a joint business
    day, otherwise the first joint business day after it.

40. `occurrences(region, rule, start, end)` -> list of days: rules 22 to
    28. If `start` is later than `end` the result is the empty list (after
    the arguments have been checked by rule 29).

41. Speed: every call returns in under one second for any arguments
    allowed by rule 1, for example `add_business_days` with `n` of 30000
    or -30000 from a day in the middle of the range, `business_days_between`
    from 1900-01-01 to 2199-12-31, and `occurrences` of a daily rule anchored
    in 1900 queried for a range in 2199.
