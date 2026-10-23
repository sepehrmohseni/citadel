import { describe, expect, it, vi } from "vitest";
import { bytes, duration, fuzzy, h, rate } from "../dom";

describe("h()", () => {
  it("builds elements with attributes, properties, datasets and handlers", () => {
    const click = vi.fn();
    const el = h("button", { class: "btn", type: "button", disabled: true, dataset: { id: "x1" }, onClick: click }, "go", 3);
    expect(el.className).toBe("btn");
    expect(el.disabled).toBe(true);
    expect(el.dataset.id).toBe("x1");
    expect(el.textContent).toBe("go3");
    el.disabled = false;
    el.click();
    expect(click).toHaveBeenCalledOnce();
  });

  it("creates svg in the svg namespace and skips falsy children", () => {
    const svg = h("svg", { viewBox: "0 0 1 1" }, h("path", { d: "M0 0" }), null, false, undefined);
    expect(svg.namespaceURI).toBe("http://www.w3.org/2000/svg");
    expect(svg.getAttribute("viewBox")).toBe("0 0 1 1");
    expect(svg.childNodes.length).toBe(1);
  });
});

describe("formatting", () => {
  it("formats byte counts with binary units", () => {
    expect(bytes(0)).toBe("0 B");
    expect(bytes(1023)).toBe("1023 B");
    expect(bytes(1536)).toBe("1.5 KiB");
    expect(bytes(5 * 1024 ** 3)).toBe("5.0 GiB");
    expect(bytes(-1)).toBe("–");
    expect(bytes(Number.NaN)).toBe("–");
    expect(rate(2048)).toBe("2.0 KiB/s");
  });

  it("formats durations coarsely", () => {
    expect(duration(59)).toBe("0m");
    expect(duration(3 * 3600 + 120)).toBe("3h 2m");
    expect(duration(2 * 86400 + 5 * 3600)).toBe("2d 5h");
  });
});

describe("fuzzy()", () => {
  it("prefers contiguous and earlier matches", () => {
    expect(fuzzy("", "anything")).toBeGreaterThan(0);
    expect(fuzzy("db", "prod-db-1")).toBeGreaterThan(fuzzy("db", "d-something-b"));
    expect(fuzzy("prod", "prod-db")).toBeGreaterThan(fuzzy("prod", "my-prod-db"));
    expect(fuzzy("xyz", "prod-db")).toBe(-1);
    expect(fuzzy("PDB", "prod-db")).toBeGreaterThan(0);
  });
});
