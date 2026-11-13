import copy
from merge import merge

assert merge({"a": 1}, {"b": 2}) == {"a": 1, "b": 2}
assert merge({"a": {"x": 1, "y": 2}}, {"a": {"y": 3, "z": 4}}) == {"a": {"x": 1, "y": 3, "z": 4}}
assert merge({"a": 1, "b": 2}, {"a": None, "missing": None}) == {"b": 2}
assert merge({"a": {"b": {"c": 1, "d": 2}}}, {"a": {"b": {"c": None}}}) == {"a": {"b": {"d": 2}}}
assert merge({"db": {"host": "h", "port": 1}}, {"db!": {"host": "x"}}) == {"db": {"host": "x"}}
assert merge({}, {"new!": [1]}) == {"new": [1]}
assert merge({"l": [1, 2]}, {"l": ["+", 3]}) == {"l": [1, 2, 3]}
assert merge({"l": [1, 2]}, {"l": [3]}) == {"l": [3]}
assert merge({"l": [1, 2]}, {"l": []}) == {"l": []}
assert merge([1], ["+"]) == [1]
assert merge({"a": {"x": 1}}, {"a": 5}) == {"a": 5}
assert merge({"a": 5}, {"a": {"x": 1}}) == {"a": {"x": 1}}
assert merge(1, 2) == 2
assert merge({"a": 1}, {}) == {"a": 1}

base = {"a": {"x": [1]}, "l": [1]}
override = {"a": {"y": [2]}, "l": ["+", [9]]}
before = copy.deepcopy((base, override))
out = merge(base, override)
assert (base, override) == before, "arguments were modified"
out["a"]["x"].append(0); out["a"]["y"].append(0); out["l"][0] = 7; out["l"][1].append(0)
assert (base, override) == before, "the result shares objects with the arguments"
print("ok")
