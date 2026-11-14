import json
import os
import shutil
import tempfile
import unittest

from contacts import search, store
from contacts.migrate import migrate_dir

ROOT = os.path.dirname(os.path.abspath(__file__))
FIXTURES = os.path.join(ROOT, "fixtures")


def read_bytes(path):
    with open(path, "rb") as handle:
        return handle.read()


def read_json(path):
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def where(problems):
    """The (file, index) of every problem, sorted, None first."""
    return sorted(((p["file"], p["index"]) for p in problems),
                  key=lambda pair: (pair[0], -1 if pair[1] is None else pair[1]))


TURING = {"id": 1, "given_name": "Alan Mathison", "family_name": "Turing",
          "email": "alan@example.org"}
HYPATIA = {"id": 2, "given_name": "Hypatia", "family_name": "",
           "email": "hypatia@example.org", "phone": "555-0002"}
NOETHER = {"id": 3, "given_name": "Emmy", "family_name": "Noether",
           "email": "emmy@example.org"}
ARISTOTLE = {"id": 4, "given_name": "Aristotle", "family_name": "",
             "email": "ari@example.org", "nick": "The Philosopher"}
FRANKLIN = {"id": 5, "given_name": "Rosalind", "family_name": "Franklin",
            "email": "rf@example.org"}
HODGKIN = {"id": 10, "given_name": "Dorothy Crowfoot", "family_name": "Hodgkin",
           "email": "dh@example.org"}
JEMISON = {"id": 13, "given_name": "Mae", "family_name": "Jemison",
           "email": "mae@example.org"}

MIXED_PROBLEMS = [
    ("03_bad.json", 1), ("03_bad.json", 2), ("03_bad.json", 3),
    ("03_bad.json", 4), ("03_bad.json", 5),
    ("04_broken.json", None), ("05_future.json", None),
    ("06_newbad.json", 1), ("06_newbad.json", 2),
    ("07_list.json", None), ("08_norecords.json", None),
]


class Case(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp, True)

    def copy(self, name):
        target = os.path.join(self.tmp, name)
        shutil.copytree(os.path.join(FIXTURES, name), target)
        return target

    def assert_problems(self, problems, expected):
        self.assertEqual(where(problems), expected)
        for problem in problems:
            self.assertEqual(set(problem), {"file", "index", "reason"})
            self.assertIsInstance(problem["reason"], str)
            self.assertTrue(problem["reason"].strip())


class LoadVersion1(Case):
    def test_untouched_v1_file(self):
        path = os.path.join(FIXTURES, "legacy", "a.json")
        before = read_bytes(path)
        records, problems = store.load_file(path)
        self.assertEqual(problems, [])
        self.assertEqual(records, [
            {"id": 101, "given_name": "Katherine", "family_name": "Johnson",
             "email": "kj@example.org", "phone": "555-0001"},
            {"id": 102, "given_name": "Cher", "family_name": "",
             "email": "cher@example.org"},
        ])
        self.assertEqual(read_bytes(path), before, "loading must not write")

    def test_name_splitting_and_carried_keys(self):
        records, problems = store.load_file(os.path.join(FIXTURES, "legacy", "b.json"))
        self.assertEqual(problems, [])
        self.assertEqual(records, [
            {"id": 103, "given_name": "Gabriel García", "family_name": "Márquez",
             "email": "GGM@Example.org", "tags": ["author", "vip"], "phone": None},
            {"id": 104, "given_name": "Ludwig van", "family_name": "Beethoven",
             "email": ""},
        ])

    def test_no_v1_keys_left(self):
        records, _ = store.load_dir(os.path.join(FIXTURES, "legacy"))
        self.assertEqual([r["id"] for r in records], [101, 102, 103, 104])
        for record in records:
            self.assertNotIn("name", record)
            self.assertNotIn("mail", record)
        self.assertNotIn("phone", records[1])
        self.assertNotIn("phone", records[3])


class LoadVersion2AndSave(Case):
    def test_v2_file_is_returned_as_is(self):
        records, problems = store.load_file(os.path.join(FIXTURES, "mixed", "02_new.json"))
        self.assertEqual(problems, [])
        self.assertEqual(records, [NOETHER, ARISTOTLE])

    def test_save_writes_version_2(self):
        path = os.path.join(self.tmp, "out.json")
        store.save_file(path, [NOETHER, HYPATIA])
        document = read_json(path)
        self.assertEqual(document["version"], 2)
        self.assertEqual(document["records"], [NOETHER, HYPATIA])
        self.assertEqual(set(document), {"version", "records"})
        self.assertEqual(store.load_file(path), ([NOETHER, HYPATIA], []))


class Problems(Case):
    def test_malformed_v1_records_are_reported_and_the_rest_loaded(self):
        records, problems = store.load_file(os.path.join(FIXTURES, "mixed", "03_bad.json"))
        self.assertEqual(records, [FRANKLIN, HODGKIN])
        self.assert_problems(problems, [("03_bad.json", i) for i in (1, 2, 3, 4, 5)])

    def test_malformed_v2_records(self):
        records, problems = store.load_file(os.path.join(FIXTURES, "mixed", "06_newbad.json"))
        self.assertEqual(records, [JEMISON])
        self.assert_problems(problems, [("06_newbad.json", 1), ("06_newbad.json", 2)])

    def test_unreadable_files(self):
        for name in ("04_broken.json", "05_future.json", "07_list.json",
                     "08_norecords.json"):
            records, problems = store.load_file(os.path.join(FIXTURES, "mixed", name))
            self.assertEqual(records, [], name)
            self.assert_problems(problems, [(name, None)])

    def test_mixed_directory(self):
        records, problems = store.load_dir(os.path.join(FIXTURES, "mixed"))
        self.assertEqual(records, [TURING, HYPATIA, NOETHER, ARISTOTLE,
                                   FRANKLIN, HODGKIN, JEMISON])
        self.assert_problems(problems, MIXED_PROBLEMS)


class Migration(Case):
    def test_legacy_directory(self):
        directory = self.copy("legacy")
        expected, _ = store.load_dir(directory)
        report = migrate_dir(directory)
        self.assertEqual(report["migrated"], ["a.json", "b.json"])
        self.assertEqual(report["current"], [])
        self.assertEqual(report["failed"], [])
        self.assertEqual(report["problems"], [])
        self.assertEqual(set(report), {"migrated", "current", "failed", "problems"})
        for name in ("a.json", "b.json"):
            self.assertEqual(read_json(os.path.join(directory, name))["version"], 2)
        document = read_json(os.path.join(directory, "a.json"))
        self.assertEqual(document["records"], expected[:2])
        self.assertEqual(store.load_dir(directory), (expected, []))

    def test_mixed_directory(self):
        directory = self.copy("mixed")
        names = sorted(os.listdir(directory))
        before = {name: read_bytes(os.path.join(directory, name)) for name in names}
        loaded_before, _ = store.load_dir(directory)

        report = migrate_dir(directory)
        self.assertEqual(report["migrated"], ["01_old.json"])
        self.assertEqual(report["current"], ["02_new.json"])
        self.assertEqual(report["failed"], [
            "03_bad.json", "04_broken.json", "05_future.json",
            "06_newbad.json", "07_list.json", "08_norecords.json"])
        self.assert_problems(report["problems"], MIXED_PROBLEMS)

        self.assertEqual(sorted(os.listdir(directory)), names,
                         "no file may appear or disappear")
        for name in names:
            if name != "01_old.json":
                self.assertEqual(read_bytes(os.path.join(directory, name)),
                                 before[name], "%s was changed" % name)
        document = read_json(os.path.join(directory, "01_old.json"))
        self.assertEqual(document["version"], 2)
        self.assertEqual(document["records"], [TURING, HYPATIA])

        loaded_after, problems_after = store.load_dir(directory)
        self.assertEqual(loaded_after, loaded_before)
        self.assert_problems(problems_after, MIXED_PROBLEMS)

    def test_second_run_migrates_nothing(self):
        directory = self.copy("mixed")
        migrate_dir(directory)
        names = sorted(os.listdir(directory))
        before = {name: read_bytes(os.path.join(directory, name)) for name in names}
        report = migrate_dir(directory)
        self.assertEqual(report["migrated"], [])
        self.assertEqual(report["current"], ["01_old.json", "02_new.json"])
        self.assertEqual(len(report["failed"]), 6)
        for name in names:
            self.assertEqual(read_bytes(os.path.join(directory, name)), before[name])

    def test_empty_directory(self):
        report = migrate_dir(self.tmp)
        self.assertEqual(report, {"migrated": [], "current": [], "failed": [],
                                  "problems": []})


class ShippedData(Case):
    """data/ must have been migrated as far as the rules allow."""

    def test_team(self):
        document = read_json(os.path.join(ROOT, "data", "team.json"))
        self.assertEqual(document["version"], 2)
        self.assertEqual(document["records"], [
            {"id": 1, "given_name": "Ada", "family_name": "Lovelace",
             "email": "ada@example.org", "phone": "555-0101"},
            {"id": 2, "given_name": "Jean-Luc van der", "family_name": "Berg",
             "email": "JL@example.org"},
            {"id": 3, "given_name": "Grace Brewster", "family_name": "Hopper",
             "email": "grace@example.org", "notes": "prefers email"},
        ])

    def test_clients(self):
        document = read_json(os.path.join(ROOT, "data", "clients.json"))
        self.assertEqual(document["version"], 2)
        self.assertEqual(document["records"], [
            {"id": 10, "given_name": "Plato", "family_name": "",
             "email": "plato@example.com"},
            {"id": 11, "given_name": "Mary Ann", "family_name": "Evans",
             "email": "george@example.com", "phone": "555-0177"},
        ])

    def test_vendors_has_a_problem_and_is_untouched(self):
        self.assertEqual(read_bytes(os.path.join(ROOT, "data", "vendors.json")),
                         read_bytes(os.path.join(FIXTURES, "original_vendors.json")))

    def test_whole_directory_loads(self):
        self.assertEqual(sorted(n for n in os.listdir(os.path.join(ROOT, "data"))
                                if n.endswith(".json")),
                         ["clients.json", "team.json", "vendors.json"])
        records, problems = store.load_dir(os.path.join(ROOT, "data"))
        self.assertEqual([r["id"] for r in records], [10, 11, 1, 2, 3, 20, 22])
        self.assertEqual(records[5], {
            "id": 20, "given_name": "Ole Kirk", "family_name": "Christiansen",
            "email": "ole@example.net"})
        self.assert_problems(problems, [("vendors.json", 1)])


class Search(unittest.TestCase):
    def test_find_by_email(self):
        records = [TURING, HYPATIA, NOETHER]
        self.assertEqual(search.find_by_email(records, "EMMY@example.ORG"), [NOETHER])
        self.assertEqual(search.find_by_email(records, "nobody@example.org"), [])

    def test_label(self):
        self.assertEqual(search.label(TURING), "Turing, Alan Mathison")
        self.assertEqual(search.label(HYPATIA), "Hypatia")

    def test_by_name(self):
        lower = {"id": 30, "given_name": "anna", "family_name": "franklin",
                 "email": "a@example.org"}
        twin = dict(FRANKLIN, id=2)
        ordered = search.by_name([TURING, FRANKLIN, NOETHER, HYPATIA, lower,
                                  ARISTOTLE, twin])
        self.assertEqual([r["id"] for r in ordered], [4, 2, 30, 2, 5, 3, 1])


if __name__ == "__main__":
    unittest.main()
