/**
 * Android keyboards (Gboard, Samsung and most others) hold typed letters back as an unfinished
 * word until space or enter. A terminal cannot work like that: Ctrl+B and then S reach tmux as
 * Ctrl+B alone, with the S stuck in the keyboard. An input field that has been a password field is
 * offered to the keyboard as a visible password, and keyboards never hold those back (Termux uses
 * the same keyboard mode). On Android the keyboard therefore types into such a field, kept out of
 * sight, and every key is passed to the terminal the moment it is pressed.
 */

/** Kept in the field so Backspace always has something to delete and reports itself. */
export const SENTINEL = "  ";

export interface DirectKeys {
  el: HTMLInputElement;
  focus(): void;
  dispose(): void;
}

export interface DirectKeysOptions {
  /** Where the field goes; inside the terminal element, so shortcuts treat it as the terminal. */
  host: HTMLElement;
  /** Typed text, and DEL (0x7f) for every character deleted. */
  send(text: string): void;
  /** Enter, arrows, Tab, Escape and Ctrl or Alt chords, for the terminal to translate. */
  key(e: KeyboardEvent): void;
  focusChange?(focused: boolean): void;
}

const MODIFIERS = new Set(["Shift", "Control", "Alt", "AltGraph", "Meta", "CapsLock", "Fn", "OS"]);

export function directKeys(opts: DirectKeysOptions): DirectKeys {
  const el = document.createElement("input");
  // Having been a password field is what makes Android offer the field as a visible password.
  el.type = "password";
  el.type = "text";
  el.className = "term-ime";
  el.tabIndex = -1;
  el.setAttribute("autocomplete", "off");
  el.setAttribute("autocorrect", "off");
  el.setAttribute("autocapitalize", "off");
  el.setAttribute("spellcheck", "false");
  el.setAttribute("enterkeyhint", "enter");
  el.setAttribute("aria-label", "Terminal input");
  el.setAttribute("data-1p-ignore", "");
  el.setAttribute("data-lpignore", "true");

  let composing = false;

  const settle = () => {
    if (el.value !== SENTINEL) el.value = SENTINEL;
    try {
      el.setSelectionRange(SENTINEL.length, SENTINEL.length);
    } catch {
      // Not focusable yet; the next settle puts the caret back.
    }
  };

  const flush = () => {
    const v = el.value;
    if (v.length > SENTINEL.length && v.startsWith(SENTINEL)) opts.send(v.slice(SENTINEL.length));
    else if (v.length < SENTINEL.length) opts.send("\x7f".repeat(SENTINEL.length - v.length));
    else if (v !== SENTINEL) opts.send(v.replace(/^ +/, ""));
    settle();
  };

  const onInput = (e: Event) => {
    // A keyboard that composes even here: wait for the finished word.
    if (composing || (e as InputEvent).isComposing) return;
    flush();
  };
  const onCompositionStart = () => {
    composing = true;
  };
  const onCompositionEnd = () => {
    composing = false;
    flush();
  };
  const onKeyDown = (e: KeyboardEvent) => {
    if (e.isComposing || e.keyCode === 229 || e.key === "Unidentified" || e.key === "Process" || e.key === "Dead") return;
    if (MODIFIERS.has(e.key)) return;
    const printable = [...e.key].length === 1;
    // Plain characters arrive as text through the input event; so do AltGr characters.
    if (printable && !e.ctrlKey && !e.altKey && !e.metaKey) return;
    if (printable && e.ctrlKey && e.altKey) return;
    e.preventDefault();
    e.stopPropagation();
    opts.key(e);
  };
  const onFocus = () => {
    settle();
    opts.focusChange?.(true);
  };
  const onBlur = () => opts.focusChange?.(false);

  el.addEventListener("input", onInput);
  el.addEventListener("compositionstart", onCompositionStart);
  el.addEventListener("compositionend", onCompositionEnd);
  el.addEventListener("keydown", onKeyDown);
  el.addEventListener("focus", onFocus);
  el.addEventListener("blur", onBlur);
  el.value = SENTINEL;
  opts.host.append(el);

  return {
    el,
    focus: () => el.focus({ preventScroll: true }),
    dispose: () => {
      el.removeEventListener("input", onInput);
      el.removeEventListener("compositionstart", onCompositionStart);
      el.removeEventListener("compositionend", onCompositionEnd);
      el.removeEventListener("keydown", onKeyDown);
      el.removeEventListener("focus", onFocus);
      el.removeEventListener("blur", onBlur);
      el.remove();
    },
  };
}

/**
 * A copy of a key event for xterm's own keyboard handling, which turns Enter, arrows (in the
 * terminal's cursor mode), function keys and Ctrl or Alt chords into the right bytes.
 */
export function cloneKey(e: KeyboardEvent): KeyboardEvent {
  const copy = new KeyboardEvent("keydown", {
    key: e.key,
    code: e.code,
    location: e.location,
    ctrlKey: e.ctrlKey,
    altKey: e.altKey,
    shiftKey: e.shiftKey,
    metaKey: e.metaKey,
    repeat: e.repeat,
    bubbles: true,
    cancelable: true,
  });
  // The constructor cannot set these, and xterm reads them.
  Object.defineProperty(copy, "keyCode", { value: e.keyCode });
  Object.defineProperty(copy, "which", { value: e.which });
  return copy;
}
