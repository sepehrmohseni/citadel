import { describe, expect, it, vi } from "vitest";
import { h } from "../dom";
import { actionToast, field, segmented, toggle } from "../ui";

describe("field()", () => {
  it("labels single form controls", () => {
    const el = field("Hostname", h("input"));
    expect(el.tagName).toBe("LABEL");
  });

  it("does not wrap compound controls in a label, so clicks reach the intended button", () => {
    const seen: string[] = [];
    const seg = segmented(
      [
        { value: "key", label: "Key" },
        { value: "password", label: "Password" },
      ],
      "key",
      (v) => seen.push(v),
    );
    const el = field("Authentication", seg);
    document.body.append(el);
    expect(el.tagName).toBe("DIV");
    expect(el.getAttribute("role")).toBe("group");
    const password = [...seg.querySelectorAll("button")].find((b) => b.textContent === "Password");
    password?.click();
    expect(seen).toEqual(["password"]);
    const on = seg.querySelector("button.on");
    expect(on?.textContent).toBe("Password");
    el.remove();
  });
});

describe("toggle()", () => {
  it("reports changes", () => {
    const fn = vi.fn();
    const t = toggle(false, "Forward agent", fn);
    const cb = t.querySelector("input") as HTMLInputElement;
    cb.checked = true;
    cb.dispatchEvent(new Event("change"));
    expect(fn).toHaveBeenCalledWith(true);
  });
});

describe("actionToast()", () => {
  it("puts the buttons on their own row under the message", () => {
    const run = vi.fn();
    actionToast("Citadel 9.9.9 is available.", [
      { label: "download", run },
      { label: "skip this version", run: vi.fn() },
    ]);
    const el = document.querySelector(".toast-action") as HTMLElement;
    expect([...el.children].map((c) => c.className)).toEqual(["lvl", "ts", "msg", "btn small ghost toast-x", "toast-acts"]);
    expect(el.querySelector(".msg")?.textContent).toBe("Citadel 9.9.9 is available.");
    const buttons = [...el.querySelectorAll(".toast-acts button")];
    expect(buttons.map((b) => b.textContent)).toEqual(["download", "skip this version"]);
    (buttons[0] as HTMLButtonElement).click();
    expect(run).toHaveBeenCalledOnce();
  });
});
