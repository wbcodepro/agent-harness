import { describe, expect, it } from "vitest";
import { enforcedDescription, enforcedLabel } from "./Header";

describe("enforced asking", () => {
  it("is shown instead of the mode, with the sandbox state", () => {
    expect(enforcedLabel("ask")).toBe("Asks first");
    expect(enforcedLabel("ask-without-sandbox")).toBe("Asks first · no sandbox");
    expect(enforcedDescription("ask")).toContain("whatever the mode");
    expect(enforcedDescription("ask-without-sandbox")).toContain("without the sandbox");
  });
});
