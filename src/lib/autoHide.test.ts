import { describe, expect, it } from "vitest";
import { AutoHide, LINGER_MS } from "./autoHide";

describe("AutoHide", () => {
  it("is always visible when the option is off", () => {
    const a = new AutoHide();
    expect(a.visible(false, 0, 0)).toBe(true);
    expect(a.visible(false, 0, 100_000)).toBe(true);
  });

  it("is hidden without cubes and appears at once with one", () => {
    const a = new AutoHide();
    expect(a.visible(true, 0, 1000)).toBe(false);
    expect(a.visible(true, 1, 1016)).toBe(true);
  });

  it("stays a moment after the last cube left, then hides", () => {
    const a = new AutoHide();
    a.visible(true, 2, 5000);
    expect(a.visible(true, 0, 5000 + LINGER_MS - 1)).toBe(true);
    expect(a.visible(true, 0, 5000 + LINGER_MS + 1)).toBe(false);
  });

  it("remembers cubes seen while the option was off", () => {
    const a = new AutoHide();
    a.visible(false, 1, 1000);
    expect(a.visible(true, 0, 1100)).toBe(true);
    expect(a.visible(true, 0, 1000 + LINGER_MS + 1)).toBe(false);
  });

  it("can be revealed for a while without cubes", () => {
    const a = new AutoHide();
    a.reveal(0, 2000);
    expect(a.visible(true, 0, 1999)).toBe(true);
    expect(a.visible(true, 0, 2001)).toBe(false);
  });
});
