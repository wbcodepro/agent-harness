import { describe, expect, it } from "vitest";
import { BlobUrls } from "./blobs";
import { splitBlocks } from "./blocks";
import { clip } from "./clip";
import { ENTER_GRACE_MS, approvalKey } from "./keys";

describe("clipping output", () => {
  it("keeps short text whole and cuts long text by lines and characters", () => {
    expect(clip("a\nb")).toEqual({ text: "a\nb", hiddenLines: 0, clipped: false });
    const long = Array.from({ length: 100 }, (_, n) => `line ${n}`).join("\n");
    const cut = clip(long, 10);
    expect(cut.clipped).toBe(true);
    expect(cut.text.split("\n")).toHaveLength(10);
    expect(cut.hiddenLines).toBe(90);
    expect(clip("x".repeat(10_000), 40, 100).text).toHaveLength(100);
  });
});

describe("splitting streamed Markdown", () => {
  it("cuts at blank lines but never inside a code fence", () => {
    expect(splitBlocks("one\n\ntwo")).toEqual(["one", "two"]);
    expect(splitBlocks("a\n\n```\ncode\n\nmore\n```\n\nb")).toEqual(["a", "```\ncode\n\nmore\n```", "b"]);
    expect(splitBlocks("open\n\n```js\nstill")).toEqual(["open", "```js\nstill"]);
    expect(splitBlocks("")).toEqual([""]);
  });
});

describe("approval keys", () => {
  it("ignores Enter right after the card appears and when held", () => {
    expect(approvalKey({ key: "Enter" }, 1000, 1000 + ENTER_GRACE_MS - 1)).toBeNull();
    expect(approvalKey({ key: "Enter" }, 1000, 1000 + ENTER_GRACE_MS)).toBe("allow");
    expect(approvalKey({ key: "Enter", repeat: true }, 0, 10_000)).toBeNull();
    expect(approvalKey({ key: "Enter", targetTag: "BUTTON" }, 0, 10_000)).toBeNull();
    expect(approvalKey({ key: "Enter", shiftKey: true }, 0, 10_000)).toBeNull();
    expect(approvalKey({ key: "Escape" }, 1000, 1000)).toBe("deny");
    expect(approvalKey({ key: "Escape", metaKey: true }, 0, 10_000)).toBeNull();
  });
});

describe("blob URLs", () => {
  it("fetches once and revokes every URL on clear", async () => {
    let made = 0;
    const revoked: string[] = [];
    const urls = new BlobUrls(
      () => `blob:${++made}`,
      (url) => revoked.push(url),
    );
    let fetched = 0;
    const fetch = async () => {
      fetched++;
      return new Blob(["x"]);
    };
    expect(await urls.get("a", fetch)).toBe("blob:1");
    expect(await urls.get("a", fetch)).toBe("blob:1");
    expect(await urls.get("b", fetch)).toBe("blob:2");
    expect(fetched).toBe(2);
    urls.clear();
    expect(revoked).toEqual(["blob:1", "blob:2"]);
    expect(await urls.get("a", fetch)).toBe("blob:3");
  });
});
