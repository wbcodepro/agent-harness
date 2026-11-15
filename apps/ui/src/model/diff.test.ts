import { describe, expect, it } from "vitest";
import { diffsFromToolInput, lineDiff, parsePatch, toHunks } from "./diff";
import { describe as describeCall, summarize } from "./tools";

describe("line diffs", () => {
  it("keeps common lines and marks the changed ones", () => {
    const lines = lineDiff("a\nb\nc\n", "a\nx\nc\n");
    expect(lines.filter((line) => line.kind === "add").map((line) => line.text)).toEqual(["x"]);
    expect(lines.filter((line) => line.kind === "del").map((line) => line.text)).toEqual(["b"]);
    expect(lines[0]).toEqual({ kind: "context", text: "a" });
    expect(lines[lines.length - 1]).toEqual({ kind: "context", text: "c" });
  });

  it("splits far-apart changes into hunks with three lines of context", () => {
    const before = Array.from({ length: 30 }, (_, n) => `line ${n}`).join("\n");
    const after = before.replace("line 2", "LINE 2").replace("line 25", "LINE 25");
    const hunks = toHunks(lineDiff(before, after));
    expect(hunks).toHaveLength(2);
    expect(hunks[0].lines[0]).toEqual({ kind: "context", text: "line 0" });
    expect(hunks[0].lines.filter((line) => line.kind === "context")).toHaveLength(5);
    expect(hunks[1].lines.filter((line) => line.kind === "context")).toHaveLength(6);
  });

  it("reads the apply_patch format", () => {
    const files = parsePatch(
      [
        "*** Begin Patch",
        "*** Update File: src/a.rs",
        "@@ fn main",
        " fn main() {",
        "-    old();",
        "+    new();",
        " }",
        "*** Add File: notes.md",
        "+# Notes",
        "*** Delete File: old.txt",
        "*** Update File: b.rs",
        "*** Move to: c.rs",
        "@@",
        "-x",
        "+y",
        "*** End Patch",
      ].join("\n"),
    );
    expect(files.map((file) => [file.path, file.op, file.added, file.removed])).toEqual([
      ["src/a.rs", "edit", 1, 1],
      ["notes.md", "add", 1, 0],
      ["old.txt", "delete", 0, 0],
      ["b.rs", "move", 1, 1],
    ]);
    expect(files[0].hunks[0].header).toBe("fn main");
    expect(files[3].movedTo).toBe("c.rs");
  });

  it("derives diffs from each edit tool's input", () => {
    expect(diffsFromToolInput("edit", { path: "a", old_string: "x\n", new_string: "y\nz\n" })[0]).toMatchObject({ added: 2, removed: 1 });
    expect(diffsFromToolInput("write", { path: "a", content: "1\n2\n3" })[0]).toMatchObject({ op: "write", added: 3 });
    expect(diffsFromToolInput("edit_lines", { path: "a", edits: [{ from: "3:abc", to: "5:def", new_text: "q" }] })[0]).toMatchObject({
      op: "lines",
      added: 1,
      removed: 3,
      hunks: [{ header: "lines 3–5" }],
    });
    expect(diffsFromToolInput("bash", { command: "ls" })).toEqual([]);
    expect(diffsFromToolInput("edit", "not an object")).toEqual([]);
  });
});

describe("tool descriptions", () => {
  it("describes calls in one line", () => {
    expect(describeCall("read", { path: "/x/src/main.rs" })).toMatchObject({ title: "Read main.rs" });
    expect(describeCall("bash", { command: "npm test\necho done" })).toMatchObject({ title: "Ran", detail: "npm test" });
    expect(describeCall("mcp__shop__price", {})).toMatchObject({ title: "shop: price" });
  });

  it("summarizes a group, counting files once", () => {
    expect(
      summarize([
        { category: "read", path: "a" },
        { category: "read", path: "a" },
        { category: "read", path: "b" },
        { category: "run" },
        { category: "run" },
        { category: "edit", path: "a" },
      ]),
    ).toBe("Read 2 files, edited 1, ran 2 commands");
    expect(summarize([], 2)).toBe("Thought");
    expect(summarize([{ category: "search" }])).toBe("Searched 1 time");
  });
});
