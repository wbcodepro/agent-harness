const assert = require("assert");
const { createQueue } = require("./queue");

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

(async () => {
  const q = createQueue(2);
  let running = 0, peak = 0;
  const started = [];
  const job = (name, ms, fail) => async () => {
    started.push(name);
    running++; peak = Math.max(peak, running);
    await sleep(ms);
    running--;
    if (fail) throw new Error("boom " + name);
    return name.toUpperCase();
  };
  const results = [
    q.add(job("a", 30)), q.add(job("b", 10)), q.add(job("c", 10, true)),
    q.add(job("d", 10)), q.add(job("e", 5)),
  ];
  const settled = await Promise.allSettled(results);
  assert.deepStrictEqual(settled.map((s) => s.status), ["fulfilled", "fulfilled", "rejected", "fulfilled", "fulfilled"]);
  assert.deepStrictEqual(settled.filter((s) => s.value).map((s) => s.value), ["A", "B", "D", "E"]);
  assert.strictEqual(settled[2].reason.message, "boom c");
  assert.strictEqual(peak, 2, "peak concurrency was " + peak);
  assert.deepStrictEqual(started, ["a", "b", "c", "d", "e"]);
  await q.idle();
  assert.strictEqual(running, 0);
  // A synchronous throw inside a task is a rejection, not a crash.
  await assert.rejects(q.add(() => { throw new Error("sync"); }), /sync/);
  await q.idle();
  // The queue is still usable afterwards.
  assert.strictEqual(await q.add(async () => 7), 7);
  console.log("ok");
})().catch((e) => { console.error(e); process.exit(1); });
