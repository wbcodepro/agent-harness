import { describe, expect, it } from "vitest";
import { Folder, foldAll, type AssistantItem, type NoticeItem, type ToolRow, type UserItem, type WorkItem } from "./fold";
import type { SessionRecord, StreamEvent } from "./types";

let seq = 0;
let clock = 1_000;

function rec(type: SessionRecord["type"], data: unknown, ts?: number): StreamEvent {
  clock += 10;
  return { kind: "record", record: { seq: seq++, ts: ts ?? clock, type, data } as SessionRecord };
}

function reset() {
  seq = 0;
  clock = 1_000;
}

const user = (text: string) => rec("user/message", { content: [{ type: "text", text }] });
const delta = (d: unknown) => rec("assistant/delta", { delta: d });
const message = (content: unknown[], stop = "end_turn", usage?: unknown) => rec("assistant/message", { content, stop, usage });
const call = (id: string, name: string, input: unknown, ts?: number) => rec("tool/call", { call_id: id, name, input }, ts);
const result = (id: string, text: string, isError = false, ts?: number) =>
  rec("tool/result", { call_id: id, content: [{ type: "text", text }], is_error: isError }, ts);

function work(items: ReturnType<typeof foldAll>["items"]): WorkItem[] {
  return items.filter((item): item is WorkItem => item.kind === "work");
}

describe("folding a session's events", () => {
  it("streams text into one assistant item and settles it", () => {
    reset();
    const folder = new Folder();
    folder.apply({ kind: "turn", state: "started" });
    folder.apply(rec("turn/start", { turn: 1 }));
    folder.apply(user("hello"));
    folder.apply(delta({ type: "text", text: "Hel" }));
    const streaming = folder.state.items[1] as AssistantItem;
    expect(streaming).toMatchObject({ kind: "assistant", text: "Hel", streaming: true });
    folder.apply(delta({ type: "text", text: "lo" }));
    const next = folder.state.items[1] as AssistantItem;
    expect(next.text).toBe("Hello");
    expect(next).not.toBe(streaming); // a changed item is a new object
    folder.apply(message([{ type: "text", text: "Hello" }], "end_turn", { input_tokens: 10, output_tokens: 2 }));
    folder.apply(rec("turn/end", { turn: 1, reason: "completed" }));
    folder.apply({ kind: "turn", state: "ended", error: null });
    const state = folder.state;
    expect(state.running).toBe(false);
    expect(state.items.map((item) => item.kind)).toEqual(["user", "assistant", "footer"]);
    expect((state.items[1] as AssistantItem).streaming).toBe(false);
    expect(state.usage.input_tokens).toBe(10);
    expect(state.items[2]).toMatchObject({ kind: "footer", turn: 1, reason: "completed", usage: { output_tokens: 2 } });
  });

  it("folds consecutive reasoning and tool calls into one group with a summary", () => {
    reset();
    const state = foldAll([
      rec("turn/start", { turn: 1 }),
      user("fix it"),
      delta({ type: "thinking", text: "Look at the code." }),
      delta({ type: "tool_use_start", id: "c1", name: "read" }),
      message([{ type: "thinking", text: "Look at the code." }, { type: "tool_use", id: "c1", name: "read", input: { path: "src/a.rs" } }], "tool_use"),
      call("c1", "read", { path: "src/a.rs" }, 2000),
      result("c1", "1\tfn main() {}", false, 2250),
      message([{ type: "tool_use", id: "c2", name: "read", input: { path: "src/b.rs" } }], "tool_use"),
      call("c2", "read", { path: "src/b.rs" }),
      result("c2", "ok"),
      call("c3", "edit", { path: "src/a.rs", old_string: "a\nb\n", new_string: "a\nc\n" }),
      result("c3", "Edited."),
      call("c4", "bash", { command: "cargo test" }),
      result("c4", "error: 1 failed\n[exit code 101]", true),
      delta({ type: "text", text: "Done." }),
      message([{ type: "text", text: "Done." }]),
      rec("turn/end", { turn: 1, reason: "completed" }),
    ]);
    expect(state.items.map((item) => item.kind)).toEqual(["user", "work", "assistant", "footer"]);
    const [group] = work(state.items);
    expect(group.rows.map((row) => row.kind)).toEqual(["reasoning", "tool", "tool", "tool", "tool"]);
    expect(group.summary).toBe("Read 2 files, edited 1, ran 1 command · 1 failed");
    expect(group.running).toBe(false);
    const read = group.rows[1] as ToolRow;
    expect(read).toMatchObject({ category: "read", title: "Read a.rs", status: "done", startTs: 2000, endTs: 2250 });
    const failed = group.rows[4] as ToolRow;
    expect(failed).toMatchObject({ category: "run", status: "failed", exitCode: 101, detail: "cargo test" });
    expect(state.commands).toEqual([expect.objectContaining({ command: "cargo test", exitCode: 101, running: false })]);
  });

  it("keeps a group live while the turn runs and shows the latest call", () => {
    reset();
    const state = foldAll([
      rec("turn/start", { turn: 1 }),
      user("go"),
      call("c1", "grep", { pattern: "TODO" }),
      result("c1", "a.rs:1"),
      call("c2", "bash", { command: "npm test" }),
    ]);
    const [group] = work(state.items);
    expect(group.running).toBe(true);
    expect(group.live).toBe("Ran npm test");
    expect((group.rows[1] as ToolRow).status).toBe("running");
  });

  it("starts a new group after assistant text", () => {
    reset();
    const state = foldAll([
      rec("turn/start", { turn: 1 }),
      user("go"),
      call("c1", "read", { path: "a" }),
      result("c1", "x"),
      message([{ type: "text", text: "Now the tests." }], "tool_use"),
      call("c2", "bash", { command: "make test" }),
      result("c2", "ok"),
      rec("turn/end", { turn: 1, reason: "completed" }),
    ]);
    expect(state.items.map((item) => item.kind)).toEqual(["user", "work", "assistant", "work", "footer"]);
  });

  it("finishes a group as soon as text follows it, while the turn goes on", () => {
    reset();
    const state = foldAll([
      rec("turn/start", { turn: 1 }),
      user("go"),
      call("c1", "read", { path: "a" }, 5000),
      result("c1", "x", false, 5100),
      delta({ type: "text", text: "Now the tests." }),
      call("c2", "bash", { command: "make test" }),
    ]);
    const [first, second] = work(state.items);
    expect(first.running).toBe(false);
    expect(first.endTs).toBeGreaterThanOrEqual(5100);
    expect(second.running).toBe(true);
  });

  it("marks the call an approval waits for and clears it when answered", () => {
    reset();
    const folder = new Folder();
    for (const event of [
      { kind: "turn", state: "started" } as StreamEvent,
      rec("turn/start", { turn: 1 }),
      user("write a.txt"),
      call("c1", "write", { path: "a.txt", content: "hi\n" }),
      { kind: "approval", id: 1, question: "Allow write a.txt?", tool: "write", tool_call_id: "c1", input: { path: "a.txt" }, effect: "write" } as StreamEvent,
    ]) {
      folder.apply(event);
    }
    expect(folder.state.approvals).toHaveLength(1);
    const row = () => (work(folder.state.items)[0].rows[0] as ToolRow);
    expect(row().status).toBe("waiting");
    // The same question replayed to a reconnecting page is not doubled.
    folder.apply({ kind: "approval", id: 1, question: "Allow write a.txt?", tool_call_id: "c1" });
    expect(folder.state.approvals).toHaveLength(1);
    folder.apply({ kind: "approval_resolved", id: 1, allow: true });
    expect(folder.state.approvals).toHaveLength(0);
    expect(row().status).toBe("running");
    folder.apply(result("c1", "Wrote 3 bytes."));
    expect(row().status).toBe("done");
  });

  it("drops waiting approvals when the turn ends", () => {
    reset();
    const state = foldAll([
      { kind: "turn", state: "started" },
      { kind: "approval", id: 4, question: "Allow bash rm?" },
      { kind: "turn", state: "ended", error: null },
    ]);
    expect(state.approvals).toEqual([]);
    expect(state.running).toBe(false);
  });

  it("collects diffs of successful edits for the Changes panel only", () => {
    reset();
    const state = foldAll([
      rec("turn/start", { turn: 1 }),
      user("edit"),
      call("c1", "edit", { path: "src/lib.rs", old_string: "let a = 1;\n", new_string: "let a = 2;\n" }),
      result("c1", "Edited."),
      call("c2", "write", { path: "new.txt", content: "one\ntwo\n" }),
      result("c2", "Wrote."),
      call("c3", "edit", { path: "src/gone.rs", old_string: "x", new_string: "y" }),
      result("c3", "old_string not found", true),
      rec("turn/end", { turn: 1, reason: "completed" }),
    ]);
    expect(state.changes.map((change) => change.diff.path)).toEqual(["src/lib.rs", "new.txt"]);
    expect(state.changes[0].diff).toMatchObject({ op: "edit", added: 1, removed: 1 });
    expect(state.changes[1].diff).toMatchObject({ op: "write", added: 2, removed: 0 });
    expect(state.touched).toEqual([
      { path: "src/lib.rs", ops: ["edited"] },
      { path: "new.txt", ops: ["wrote"] },
    ]);
    // The failed edit still shows what was attempted on its row.
    const rows = work(state.items)[0].rows as ToolRow[];
    expect(rows[2].diffs).toHaveLength(1);
    expect(rows[2].status).toBe("failed");
  });

  it("shows images from tool results in the row and the Screens panel", () => {
    reset();
    const sha = "a".repeat(64);
    const state = foldAll([
      rec("turn/start", { turn: 1 }),
      user("look"),
      call("c1", "browser", { action: "screenshot" }),
      rec("tool/result", {
        call_id: "c1",
        content: [{ type: "text", text: "shot" }, { type: "image", artifact: { sha256: sha, mime: "image/png", bytes: 10 } }],
      }),
    ]);
    const row = work(state.items)[0].rows[0] as ToolRow;
    expect(row.images.map((image) => image.sha256)).toEqual([sha]);
    expect(state.screens).toEqual([expect.objectContaining({ tool: "browser", artifact: expect.objectContaining({ sha256: sha }) })]);
  });

  it("marks messages sent during a turn as steering and tracks the queue", () => {
    reset();
    const state = foldAll([
      rec("turn/start", { turn: 1 }),
      user("first"),
      { kind: "steering", queued: ["also this"] },
      user("also this"),
      { kind: "steering", queued: [] },
    ]);
    const users = state.items.filter((item): item is UserItem => item.kind === "user");
    expect(users.map((u) => u.steering)).toEqual([false, true]);
    expect(state.queued).toEqual([]);
  });

  it("turns a failed turn into one error with Continue, carrying the error text", () => {
    reset();
    const state = foldAll([
      { kind: "turn", state: "started" },
      rec("turn/start", { turn: 1 }),
      user("go"),
      delta({ type: "text", text: "Partial" }),
      rec("assistant/attempt", { content: [{ type: "text", text: "Partial" }], reason: "failed" }),
      rec("turn/end", { turn: 1, reason: "failed" }),
      { kind: "turn", state: "ended", error: "provider error: 529 overloaded" },
    ]);
    const errors = state.items.filter((item): item is NoticeItem => item.kind === "notice" && item.tone === "error");
    expect(errors).toHaveLength(1);
    expect(errors[0]).toMatchObject({ text: "provider error: 529 overloaded", action: "continue" });
    expect(state.items.find((item) => item.kind === "assistant")).toMatchObject({ cut: "failed", streaming: false });
    expect(state.error).toBe("provider error: 529 overloaded");
  });

  it("marks calls left running as interrupted when a turn is stopped", () => {
    reset();
    const state = foldAll([
      rec("turn/start", { turn: 1 }),
      user("go"),
      call("c1", "bash", { command: "sleep 100" }),
      rec("turn/end", { turn: 1, reason: "cancelled" }),
    ]);
    expect((work(state.items)[0].rows[0] as ToolRow).status).toBe("interrupted");
    expect(state.items.some((item) => item.kind === "notice" && item.text === "Stopped.")).toBe(true);
  });

  it("builds items from settled messages when no deltas were journalled", () => {
    reset();
    const state = foldAll([
      rec("turn/start", { turn: 1 }),
      user("hi"),
      message([{ type: "thinking", text: "Hmm." }, { type: "text", text: "Hi there." }]),
      rec("turn/end", { turn: 1, reason: "completed" }),
    ]);
    expect(state.items.map((item) => item.kind)).toEqual(["user", "work", "assistant", "footer"]);
    expect(work(state.items)[0].summary).toBe("Thought");
  });

  it("shows a task and its verdict, and hame's own feedback as a notice", () => {
    reset();
    const state = foldAll([
      rec("task/start", { contract: { id: "t1", outcome: "README exists", acceptance: [{ kind: "file_exists", path: "README.md" }] } }),
      rec("turn/start", { turn: 1 }),
      user("work on the task"),
      rec("user/message", { content: [{ type: "text", text: "[hame verify] README.md is missing" }] }),
      rec("task/verified", { report: { task_id: "t1", passed: true, results: [{ predicate: { kind: "file_exists", path: "README.md" }, passed: true, evidence: "found" }], ms: 3 } }),
    ]);
    expect(state.items.map((item) => item.kind)).toEqual(["task", "user", "notice", "verdict"]);
    expect(state.verification?.passed).toBe(true);
  });

  it("ignores records it has already seen (a replay after reconnecting)", () => {
    reset();
    const events = [rec("turn/start", { turn: 1 }), user("once")];
    const state = foldAll([...events, ...events]);
    expect(state.items.filter((item) => item.kind === "user")).toHaveLength(1);
    expect(state.lastSeq).toBe(1);
  });

  it("replaces the waiting approvals with the list sent on reconnecting", () => {
    reset();
    const folder = new Folder();
    folder.apply({ kind: "turn", state: "started" });
    folder.apply(rec("turn/start", { turn: 1 }));
    folder.apply(user("go"));
    folder.apply(call("c1", "write", { path: "a", content: "x" }));
    folder.apply({ kind: "approval", id: 1, question: "a?", tool_call_id: "c1" });
    folder.apply({ kind: "approval", id: 2, question: "b?", tool_call_id: "c2" });
    // The host answered question 1 while we were away; only 2 waits.
    folder.apply({ kind: "approvals", pending: [{ kind: "approval", id: 2, question: "b?", tool_call_id: "c2" }] });
    expect(folder.state.approvals.map((a) => a.id)).toEqual([2]);
    expect((work(folder.state.items)[0].rows[0] as ToolRow).status).toBe("running");
  });

  it("stops running at a turn/end record even if the turn event was missed", () => {
    reset();
    const state = foldAll([
      { kind: "turn", state: "started" },
      { kind: "approval", id: 1, question: "x?" },
      rec("turn/start", { turn: 1 }),
      user("go"),
      rec("turn/end", { turn: 1, reason: "completed" }),
    ]);
    expect(state.running).toBe(false);
    expect(state.approvals).toEqual([]);
  });

  it("keys rows by their call record, so a reused call id gets its own row", () => {
    reset();
    const state = foldAll([
      rec("turn/start", { turn: 1 }),
      user("go"),
      call("dup", "bash", { command: "echo one" }),
      result("dup", "one"),
      call("dup", "bash", { command: "echo two" }),
      result("dup", "two"),
      call("dup", "edit", { path: "a", old_string: "x", new_string: "y" }),
      result("dup", "ok"),
      rec("turn/end", { turn: 1, reason: "completed" }),
    ]);
    const rows = work(state.items)[0].rows as ToolRow[];
    expect(new Set(rows.map((row) => row.key)).size).toBe(3);
    expect(rows.map((row) => row.output)).toEqual(["one", "two", "ok"]);
    expect(rows.every((row) => row.status === "done")).toBe(true);
    expect(state.commands.map((command) => command.output)).toEqual(["one", "two"]);
    expect(new Set(state.commands.map((command) => command.key)).size).toBe(2);
    expect(state.changes).toHaveLength(1);
  });

  it("settles text, then thinking, then text without repeating the first text", () => {
    reset();
    const state = foldAll([
      rec("turn/start", { turn: 1 }),
      user("go"),
      delta({ type: "text", text: "First." }),
      delta({ type: "thinking", text: "Hmm." }),
      delta({ type: "text", text: "Second." }),
      message([
        { type: "text", text: "First." },
        { type: "thinking", text: "Hmm." },
        { type: "text", text: "Second." },
      ]),
    ]);
    const texts = state.items.filter((item): item is AssistantItem => item.kind === "assistant").map((item) => item.text);
    expect(texts).toEqual(["First.", "Second."]);
  });

  it("keeps final text that arrived without text deltas", () => {
    reset();
    const state = foldAll([
      rec("turn/start", { turn: 1 }),
      user("go"),
      delta({ type: "thinking", text: "Thinking it over." }),
      message([
        { type: "thinking", text: "Thinking it over." },
        { type: "text", text: "The answer." },
      ]),
    ]);
    expect(state.items.find((item) => item.kind === "assistant")).toMatchObject({ text: "The answer.", streaming: false });
    expect(work(state.items)[0].rows).toHaveLength(1);
  });

  it("shows a user's own message that starts with [hame as a message", () => {
    reset();
    const state = foldAll([rec("turn/start", { turn: 1 }), user("[hame verify] is a funny name")]);
    expect(state.items.map((item) => item.kind)).toEqual(["user"]);
  });

  it("shows a mode change in the transcript", () => {
    reset();
    const state = foldAll([rec("mode/changed", { mode: "ask", previous: "default", by: "phone-1", label: "Ana's phone" })]);
    expect(state.items[0]).toMatchObject({ kind: "notice", text: "Permission mode: Ask (was Sandboxed), changed by Ana's phone." });
    expect(state.mode).toBe("ask");
  });
});
