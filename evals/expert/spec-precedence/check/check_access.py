import copy
import unittest

from access import decide


def req(path="/docs/a", user="ann", groups=("staff",), action="read", now=100):
    return {"user": user, "groups": list(groups), "action": action,
            "path": path, "now": now}


def rule(rule_id, effect="allow", who="*", path="/**", **extra):
    out = {"id": rule_id, "effect": effect, "who": who, "path": path}
    out.update(extra)
    return out


class Result(unittest.TestCase):
    def test_no_rules(self):
        self.assertEqual(decide(req(), []), ("deny", None))

    def test_no_rule_applies(self):
        rules = [rule("r1", who="user:bob"), rule("r2", path="/other")]
        self.assertEqual(decide(req(), rules), ("deny", None))

    def test_single_rule(self):
        self.assertEqual(decide(req(), [rule("r1")]), ("allow", "r1"))
        self.assertEqual(decide(req(), [rule("r1", "deny")]), ("deny", "r1"))

    def test_result_is_a_tuple(self):
        self.assertIsInstance(decide(req(), [rule("r1")]), tuple)
        self.assertIsInstance(decide(req(), []), tuple)

    def test_arguments_not_modified(self):
        request = req(path="/a//b/")
        rules = [rule("r1", path="/a/*", actions=["READ"], expires=None),
                 rule("r2", "deny", "group:staff", "/a/**", final=True)]
        before = copy.deepcopy((request, rules))
        decide(request, rules)
        self.assertEqual((request, rules), before)


class Validation(unittest.TestCase):
    def assert_invalid(self, rules, rule_id, request=None):
        with self.assertRaises(ValueError) as caught:
            decide(request or req(), rules)
        self.assertIn(rule_id, str(caught.exception))

    def test_bad_effect(self):
        self.assert_invalid([rule("bad-effect", effect="permit")], "bad-effect")
        self.assert_invalid([rule("upper-effect", effect="Allow")], "upper-effect")

    def test_bad_who(self):
        for who in ("ann", "user:", "group:", "role:admin", "", "users:ann"):
            self.assert_invalid([rule("bad-who", who=who)], "bad-who")

    def test_double_star_not_last(self):
        self.assert_invalid([rule("bad-path", path="/a/**/b")], "bad-path")
        self.assert_invalid([rule("bad-path-2", path="**/b")], "bad-path-2")

    def test_invalid_rule_that_could_not_apply(self):
        rules = [rule("fine"),
                 rule("elsewhere", effect="maybe", who="user:bob", path="/zzz")]
        self.assert_invalid(rules, "elsewhere")

    def test_invalid_rule_that_is_expired(self):
        self.assert_invalid([rule("old", effect="maybe", expires=1)], "old")
        self.assert_invalid([rule("old-who", who="nobody", expires=1)], "old-who")

    def test_invalid_rule_without_any_match(self):
        self.assert_invalid([rule("lonely", who="x", path="/nowhere")], "lonely")

    def test_first_invalid_rule_is_reported(self):
        rules = [rule("ok"), rule("first-bad", effect="x"), rule("second-bad", who="y")]
        with self.assertRaises(ValueError) as caught:
            decide(req(), rules)
        self.assertIn("first-bad", str(caught.exception))
        self.assertNotIn("second-bad", str(caught.exception))

    def test_valid_forms_are_accepted(self):
        rules = [rule("a", who="user:x"), rule("b", who="group:y"),
                 rule("c", path="/**"), rule("d", path="**"), rule("e", path="/"),
                 rule("f", path="/a*/b"), rule("g", path="/*/**")]
        self.assertEqual(decide(req(path="/q"), rules), ("allow", "g"))


class Who(unittest.TestCase):
    def test_user(self):
        self.assertEqual(decide(req(), [rule("r", who="user:ann")]), ("allow", "r"))
        self.assertEqual(decide(req(), [rule("r", who="user:Ann")]), ("deny", None))
        self.assertEqual(decide(req(), [rule("r", who="user:staff")]), ("deny", None))

    def test_group(self):
        self.assertEqual(decide(req(), [rule("r", who="group:staff")]), ("allow", "r"))
        self.assertEqual(decide(req(groups=()), [rule("r", who="group:staff")]),
                         ("deny", None))
        self.assertEqual(decide(req(), [rule("r", who="group:ann")]), ("deny", None))
        self.assertEqual(decide(req(groups=("a", "b", "c")), [rule("r", who="group:c")]),
                         ("allow", "r"))


class Paths(unittest.TestCase):
    def applies(self, pattern, path):
        return decide(req(path=path), [rule("r", path=pattern)]) == ("allow", "r")

    def test_literal(self):
        self.assertTrue(self.applies("/docs/a", "/docs/a"))
        self.assertFalse(self.applies("/docs/a", "/docs/b"))
        self.assertFalse(self.applies("/docs/a", "/docs"))
        self.assertFalse(self.applies("/docs", "/docs/a"))

    def test_case_matters(self):
        self.assertFalse(self.applies("/Docs/a", "/docs/a"))
        self.assertFalse(self.applies("/docs/a", "/docs/A"))

    def test_empty_segments_are_dropped(self):
        self.assertTrue(self.applies("/docs/a", "/docs//a/"))
        self.assertTrue(self.applies("docs//a/", "/docs/a"))
        self.assertTrue(self.applies("docs/a", "docs/a"))
        self.assertTrue(self.applies("//docs/*//", "docs/a"))

    def test_single_star(self):
        self.assertTrue(self.applies("/docs/*", "/docs/a"))
        self.assertTrue(self.applies("/*/a", "/docs/a"))
        self.assertFalse(self.applies("/docs/*", "/docs"))
        self.assertFalse(self.applies("/docs/*", "/docs/a/b"))
        self.assertFalse(self.applies("/*", "/docs/a"))

    def test_star_inside_a_segment_is_literal(self):
        self.assertFalse(self.applies("/docs/a*", "/docs/abc"))
        self.assertTrue(self.applies("/docs/a*", "/docs/a*"))
        self.assertFalse(self.applies("/d*/a", "/docs/a"))
        self.assertFalse(self.applies("/docs/*.txt", "/docs/a.txt"))

    def test_double_star(self):
        self.assertTrue(self.applies("/docs/**", "/docs/a"))
        self.assertTrue(self.applies("/docs/**", "/docs/a/b/c"))
        self.assertFalse(self.applies("/docs/**", "/other/a"))
        self.assertFalse(self.applies("/docs/a/b/**", "/docs/a"))

    def test_double_star_matches_no_segments(self):
        self.assertTrue(self.applies("/docs/**", "/docs"))
        self.assertTrue(self.applies("/docs/**", "/docs/"))
        self.assertTrue(self.applies("/docs/*/**", "/docs/a"))
        self.assertFalse(self.applies("/docs/*/**", "/docs"))

    def test_root(self):
        self.assertTrue(self.applies("/", "/"))
        self.assertTrue(self.applies("/**", "/"))
        self.assertTrue(self.applies("", "/"))
        self.assertTrue(self.applies("/", ""))
        self.assertFalse(self.applies("/", "/docs"))
        self.assertFalse(self.applies("/*", "/"))
        self.assertFalse(self.applies("/docs", "/"))


class Actions(unittest.TestCase):
    def applies(self, action, **extra):
        return decide(req(action=action), [rule("r", **extra)]) == ("allow", "r")

    def test_no_actions_key_covers_everything(self):
        self.assertTrue(self.applies("anything"))

    def test_listed(self):
        self.assertTrue(self.applies("read", actions=["read", "write"]))
        self.assertTrue(self.applies("write", actions=["read", "write"]))
        self.assertFalse(self.applies("delete", actions=["read", "write"]))

    def test_letter_case_is_ignored(self):
        self.assertTrue(self.applies("READ", actions=["read"]))
        self.assertTrue(self.applies("read", actions=["Read"]))
        self.assertTrue(self.applies("wRiTe", actions=["read", "WRITE"]))

    def test_empty_list_covers_nothing(self):
        self.assertFalse(self.applies("read", actions=[]))
        self.assertFalse(self.applies("", actions=[]))

    def test_star_entry(self):
        self.assertTrue(self.applies("delete", actions=["*"]))
        self.assertTrue(self.applies("delete", actions=["read", "*"]))


class Expiry(unittest.TestCase):
    def applies(self, now, **extra):
        return decide(req(now=now), [rule("r", **extra)]) == ("allow", "r")

    def test_before_expiry(self):
        self.assertTrue(self.applies(99, expires=100))

    def test_at_expiry_is_expired(self):
        self.assertFalse(self.applies(100, expires=100))

    def test_after_expiry(self):
        self.assertFalse(self.applies(101, expires=100))

    def test_none_never_expires(self):
        self.assertTrue(self.applies(10 ** 12, expires=None))
        self.assertTrue(self.applies(10 ** 12))

    def test_zero_is_a_real_expiry(self):
        self.assertFalse(self.applies(0, expires=0))
        self.assertFalse(self.applies(5, expires=0))
        self.assertTrue(self.applies(-1, expires=0))

    def test_expired_rule_does_not_take_part(self):
        rules = [rule("live", "allow", "*", "/**"),
                 rule("dead", "deny", "user:ann", "/docs/a", expires=100, final=True)]
        self.assertEqual(decide(req(now=100), rules), ("allow", "live"))
        self.assertEqual(decide(req(now=99), rules), ("deny", "dead"))


class Precedence(unittest.TestCase):
    def both_orders(self, first, second, expected):
        self.assertEqual(decide(req(), [first, second]), expected)
        self.assertEqual(decide(req(), [second, first]), expected)

    def test_more_literal_segments_win(self):
        self.both_orders(rule("deep", "allow", "*", "/docs/a"),
                         rule("wide", "deny", "user:ann", "/docs/*"),
                         ("allow", "deep"))
        self.both_orders(rule("one", "allow", "*", "/docs/**"),
                         rule("none", "deny", "user:ann", "/*/*"),
                         ("allow", "one"))

    def test_literal_count_beats_open_end(self):
        # /docs/a/** has two literal segments, /docs/* only one.
        self.both_orders(rule("open", "allow", "*", "/docs/a/**"),
                         rule("closed", "deny", "user:ann", "/docs/*"),
                         ("allow", "open"))

    def test_closed_pattern_beats_open_one(self):
        self.both_orders(rule("closed", "allow", "*", "/docs/*"),
                         rule("open", "deny", "user:ann", "/docs/**"),
                         ("allow", "closed"))
        self.both_orders(rule("closed", "allow", "*", "/*/*"),
                         rule("open", "deny", "user:ann", "/**"),
                         ("allow", "closed"))

    def test_user_beats_group_beats_everyone(self):
        self.both_orders(rule("u", "allow", "user:ann", "/docs/*"),
                         rule("g", "deny", "group:staff", "/docs/*"),
                         ("allow", "u"))
        self.both_orders(rule("g", "allow", "group:staff", "/docs/*"),
                         rule("e", "deny", "*", "/docs/*"),
                         ("allow", "g"))
        self.both_orders(rule("u", "allow", "user:ann", "/docs/*"),
                         rule("e", "deny", "*", "/docs/*"),
                         ("allow", "u"))

    def test_deny_beats_allow(self):
        self.both_orders(rule("yes", "allow", "group:staff", "/docs/*"),
                         rule("no", "deny", "group:staff", "/docs/*"),
                         ("deny", "no"))

    def test_later_rule_wins_last(self):
        a = rule("a", "allow", "user:ann", "/docs/a")
        b = rule("b", "allow", "user:ann", "/docs/a")
        c = rule("c", "allow", "user:ann", "/docs/a")
        self.assertEqual(decide(req(), [a, b, c]), ("allow", "c"))
        self.assertEqual(decide(req(), [c, b, a]), ("allow", "a"))
        d1 = rule("d1", "deny", "*", "/**")
        d2 = rule("d2", "deny", "*", "/**")
        self.assertEqual(decide(req(), [d1, d2]), ("deny", "d2"))

    def test_position_does_not_beat_effect(self):
        rules = [rule("no", "deny", "*", "/docs/*"),
                 rule("yes", "allow", "*", "/docs/*"),
                 rule("yes2", "allow", "*", "/docs/*")]
        self.assertEqual(decide(req(), rules), ("deny", "no"))

    def test_final_rules_push_out_the_others(self):
        self.both_orders(rule("exact", "allow", "user:ann", "/docs/a"),
                         rule("lock", "deny", "*", "/**", final=True),
                         ("deny", "lock"))
        self.both_orders(rule("exact", "deny", "user:ann", "/docs/a"),
                         rule("pass", "allow", "*", "/**", final=True),
                         ("allow", "pass"))

    def test_final_false_is_not_final(self):
        self.both_orders(rule("exact", "allow", "user:ann", "/docs/a"),
                         rule("soft", "deny", "*", "/**", final=False),
                         ("allow", "exact"))

    def test_final_rule_that_does_not_apply_changes_nothing(self):
        self.both_orders(rule("exact", "allow", "user:ann", "/docs/a"),
                         rule("lock", "deny", "user:bob", "/**", final=True),
                         ("allow", "exact"))
        self.both_orders(rule("exact", "allow", "user:ann", "/docs/a"),
                         rule("lock", "deny", "*", "/**", final=True, actions=[]),
                         ("allow", "exact"))

    def test_final_rules_compete_by_the_same_criteria(self):
        rules = [rule("f-wide", "deny", "user:ann", "/**", final=True),
                 rule("f-deep", "allow", "*", "/docs/**", final=True),
                 rule("plain", "deny", "user:ann", "/docs/a")]
        self.assertEqual(decide(req(), rules), ("allow", "f-deep"))
        rules = [rule("f-allow", "allow", "group:staff", "/docs/*", final=True),
                 rule("f-deny", "deny", "group:staff", "/docs/*", final=True),
                 rule("f-allow-2", "allow", "group:staff", "/docs/*", final=True)]
        self.assertEqual(decide(req(), rules), ("deny", "f-deny"))

    def test_all_criteria_together(self):
        rules = [
            rule("everyone", "allow", "*", "/**"),
            rule("staff-docs", "allow", "group:staff", "/docs/**"),
            rule("no-write", "deny", "group:staff", "/docs/**", actions=["write"]),
            rule("ann-drafts", "allow", "user:ann", "/docs/drafts/*"),
            rule("old-freeze", "deny", "*", "/docs/**", final=True, expires=50),
            rule("secret", "deny", "*", "/docs/secret/**"),
        ]
        ask = lambda path, action, **kw: decide(req(path=path, action=action, **kw), rules)
        self.assertEqual(ask("/docs/x", "read"), ("allow", "staff-docs"))
        self.assertEqual(ask("/docs/x", "WRITE"), ("deny", "no-write"))
        self.assertEqual(ask("/docs/drafts/x", "write"), ("allow", "ann-drafts"))
        self.assertEqual(ask("/docs/drafts/x/y", "write"), ("deny", "no-write"))
        self.assertEqual(ask("/docs/secret", "read"), ("deny", "secret"))
        self.assertEqual(ask("/docs/secret/k", "read"), ("deny", "secret"))
        self.assertEqual(ask("/", "read"), ("allow", "everyone"))
        self.assertEqual(ask("/docs/x", "read", now=49), ("deny", "old-freeze"))
        self.assertEqual(ask("/docs/x", "read", user="bob", groups=()),
                         ("allow", "everyone"))
        self.assertEqual(ask("/docs/drafts/x", "write", user="bob"),
                         ("deny", "no-write"))


if __name__ == "__main__":
    unittest.main()
