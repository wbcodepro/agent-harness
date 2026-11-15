import { afterEach, beforeEach, describe, expect, it } from "vitest";

// The tests run in Node: a bare EventTarget stands in for `window`.
const target = new EventTarget();

describe("desktop shell events", () => {
  beforeEach(() => {
    (globalThis as { window?: unknown }).window = target;
  });
  afterEach(() => {
    delete (globalThis as { window?: unknown }).window;
  });

  it("are absent outside the desktop app", async () => {
    const { desktop, pickFolder } = await import("./desktop");
    expect(desktop).toBeNull();
    await expect(pickFolder()).rejects.toThrow("not in the desktop app");
  });

  it("open a well-formed session or start a new one, and nothing else", async () => {
    const { onDesktopEvent } = await import("./desktop");
    const seen: unknown[] = [];
    const stop = onDesktopEvent((event) => seen.push(event));
    const send = (detail: unknown) => target.dispatchEvent(new CustomEvent("hame:desktop", { detail }));
    send({ action: "open", session: "0193-abc_D" });
    send({ action: "new" });
    send({ action: "open", session: "../x" });
    send({ action: "open", session: 7 });
    send({ action: "delete", session: "a" });
    send(null);
    stop();
    send({ action: "new" });
    expect(seen).toEqual([{ action: "open", session: "0193-abc_D" }, { action: "new" }]);
  });
});
