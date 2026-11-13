from schema import validate

schema = {
    "type": "object",
    "required": ["user", "tags"],
    "properties": {
        "user": {"type": "object", "required": ["name"], "properties": {
            "name": {"type": "string", "maxLength": 5},
            "age": {"type": "integer", "minimum": 0}}},
        "tags": {"type": "array", "items": {"enum": ["a", "b"]}},
        "ratio": {"type": "number"},
        "on": {"type": "boolean"},
    },
}
assert validate({"user": {"name": "An", "age": 3}, "tags": ["a", "b"], "ratio": 1, "on": True}, schema) == []
errors = validate({"user": {"name": "toolong", "age": -1}, "tags": ["a", "z"], "ratio": "x", "on": 1}, schema)
starts = sorted(e.split(":")[0] for e in errors)
assert starts == ["$.on", "$.ratio", "$.tags[1]", "$.user.age", "$.user.name"], errors
assert [e.split(":")[0] for e in validate({"tags": []}, schema)] == ["$"]          # missing user
assert validate(True, {"type": "integer"}) != []                                   # bool is not an integer
assert validate(None, {"type": "null"}) == [] and validate(2.0, {"type": "number"}) == []
assert validate([1, "x"], {"type": "array", "items": {"type": "integer"}})[0].startswith("$[1]")
print("ok")
