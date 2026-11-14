const assert = require("assert");
const { Emitter } = require("./emitter");

// once fires a single time, even when the listener emits again itself.
let e = new Emitter();
let calls = 0;
e.once("x", () => { calls++; if (calls < 3) e.emit("x"); });
e.emit("x");
assert.strictEqual(calls, 1, "once listener ran " + calls + " times");

// off removes a listener that was registered with once.
e = new Emitter();
calls = 0;
const f = () => calls++;
e.once("y", f);
e.off("y", f);
e.emit("y");
assert.strictEqual(calls, 0, "off did not remove a once listener");

// Removing listeners during an emit does not skip or re-run others.
e = new Emitter();
const seen = [];
const a = () => { seen.push("a"); e.off("z", a); };
const b = () => seen.push("b");
const c = () => { seen.push("c"); e.off("z", d); };
const d = () => seen.push("d");
e.on("z", a).on("z", b).on("z", c).on("z", d);
e.emit("z");
assert.deepStrictEqual(seen, ["a", "b", "c"]);
e.emit("z");
assert.deepStrictEqual(seen, ["a", "b", "c", "b", "c"]);
assert.strictEqual(new Emitter().emit("none"), false);
console.log("ok");
