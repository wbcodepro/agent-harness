import json
import os
import tempfile
import unittest

from contacts import search, store

ADA = {"id": 1, "name": "Ada Lovelace", "mail": "ada@example.org"}
BOB = {"id": 2, "name": "bob stone", "mail": "Bob@example.org"}


class Store(unittest.TestCase):
    def test_round_trip(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "a.json")
            store.save_file(path, [ADA, BOB])
            with open(path, encoding="utf-8") as handle:
                self.assertEqual(json.load(handle)["version"], 1)
            self.assertEqual(store.load_file(path), [ADA, BOB])

    def test_load_dir_reads_files_in_name_order(self):
        with tempfile.TemporaryDirectory() as tmp:
            store.save_file(os.path.join(tmp, "b.json"), [ADA])
            store.save_file(os.path.join(tmp, "a.json"), [BOB])
            self.assertEqual(store.load_dir(tmp), [BOB, ADA])


class Search(unittest.TestCase):
    def test_find_by_email(self):
        self.assertEqual(search.find_by_email([ADA, BOB], "bob@EXAMPLE.org"), [BOB])

    def test_label_and_order(self):
        self.assertEqual(search.label(ADA), "Ada Lovelace")
        self.assertEqual(search.by_name([BOB, ADA]), [ADA, BOB])


if __name__ == "__main__":
    unittest.main()
