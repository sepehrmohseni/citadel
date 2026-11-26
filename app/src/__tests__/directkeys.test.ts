import { afterEach, describe, expect, it, vi } from "vitest";
import { SENTINEL, cloneKey, directKeys } from "../directkeys";

function setup() {
  const host = document.createElement("div");
  document.body.append(host);
  const send = vi.fn<(text: string) => void>();
  const key = vi.fn<(e: KeyboardEvent) => void>();
  const focusChange = vi.fn<(on: boolean) => void>();
  const keys = directKeys({ host, send, key, focusChange });
  const type = (value: string, init: InputEventInit = {}) => {
    keys.el.value = value;
    keys.el.dispatchEvent(new InputEvent("input", { bubbles: true, ...init }));
  };
  return { host, send, key, focusChange, keys, type };
}

afterEach(() => {
  document.body.replaceChildren();
});

describe("direct keys for Android keyboards", () => {
  it("is a text field that has been a password field, inside the terminal", () => {
    const { host, keys } = setup();
    expect(keys.el.type).toBe("text");
    expect(keys.el.getAttribute("autocomplete")).toBe("off");
    expect(keys.el.getAttribute("autocapitalize")).toBe("off");
    expect(keys.el.getAttribute("spellcheck")).toBe("false");
    expect(keys.el.parentElement).toBe(host);
    expect(keys.el.value).toBe(SENTINEL);
  });

  it("sends each typed key at once and empties the field", () => {
    const { send, keys, type } = setup();
    type(`${SENTINEL}b`, { inputType: "insertText", data: "b" });
    type(`${SENTINEL}s`, { inputType: "insertText", data: "s" });
    expect(send.mock.calls).toEqual([["b"], ["s"]]);
    expect(keys.el.value).toBe(SENTINEL);
  });

  it("turns Backspace into DEL even though the field looks empty", () => {
    const { send, type } = setup();
    type(SENTINEL.slice(1), { inputType: "deleteContentBackward" });
    expect(send.mock.calls).toEqual([["\x7f"]]);
  });

  it("waits for a keyboard that composes anyway, then sends the word", () => {
    const { send, keys, type } = setup();
    keys.el.dispatchEvent(new CompositionEvent("compositionstart"));
    type(`${SENTINEL}b`, { inputType: "insertCompositionText", data: "b", isComposing: true });
    type(`${SENTINEL}bs`, { inputType: "insertCompositionText", data: "bs", isComposing: true });
    expect(send).not.toHaveBeenCalled();
    keys.el.dispatchEvent(new CompositionEvent("compositionend", { data: "bs" }));
    expect(send.mock.calls).toEqual([["bs"]]);
    expect(keys.el.value).toBe(SENTINEL);
  });

  it("hands Enter, arrows and Ctrl chords to the terminal, and leaves letters as text", () => {
    const { key, keys } = setup();
    const press = (init: KeyboardEventInit) => {
      const e = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
      keys.el.dispatchEvent(e);
      return e;
    };
    expect(press({ key: "Enter" }).defaultPrevented).toBe(true);
    press({ key: "ArrowUp" });
    press({ key: "c", ctrlKey: true });
    press({ key: "b", altKey: true });
    expect(key.mock.calls.map(([e]) => `${e.ctrlKey ? "C-" : ""}${e.altKey ? "M-" : ""}${e.key}`)).toEqual(["Enter", "ArrowUp", "C-c", "M-b"]);
    key.mockClear();
    expect(press({ key: "a" }).defaultPrevented).toBe(false);
    press({ key: "Shift" });
    press({ key: "Unidentified", keyCode: 229 });
    press({ key: "@", ctrlKey: true, altKey: true });
    expect(key).not.toHaveBeenCalled();
  });

  it("reports focus and goes away on dispose", () => {
    const { focusChange, keys, host } = setup();
    keys.focus();
    expect(document.activeElement).toBe(keys.el);
    keys.el.blur();
    expect(focusChange.mock.calls).toEqual([[true], [false]]);
    keys.dispose();
    expect(host.contains(keys.el)).toBe(false);
  });

  it("copies key events with the key codes xterm reads", () => {
    const original = new KeyboardEvent("keydown", { key: "ArrowLeft", code: "ArrowLeft", ctrlKey: true });
    Object.defineProperty(original, "keyCode", { value: 37 });
    const copy = cloneKey(original);
    expect([copy.type, copy.key, copy.code, copy.ctrlKey, copy.keyCode, copy.bubbles]).toEqual(["keydown", "ArrowLeft", "ArrowLeft", true, 37, true]);
  });
});
