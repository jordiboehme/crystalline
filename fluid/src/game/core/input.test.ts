import { afterEach, describe, expect, it } from "vitest";

import { createInput, TYPED_CAP, type Input } from "./input";

let input: Input | null = null;
afterEach(() => {
  input?.dispose();
  input = null;
});

function key(type: "keydown" | "keyup", code: string, repeat = false) {
  window.dispatchEvent(new KeyboardEvent(type, { code, repeat }));
}

describe("createInput", () => {
  it("holds a key between down and up, by code", () => {
    input = createInput(document.createElement("canvas"));
    key("keydown", "KeyW");
    expect(input.held("KeyW")).toBe(true);
    key("keyup", "KeyW");
    expect(input.held("KeyW")).toBe(false);
  });

  it("reports a press once, and not for auto-repeat", () => {
    input = createInput(document.createElement("canvas"));
    key("keydown", "Digit1");
    key("keydown", "Digit1", true);
    expect(input.pressed("Digit1")).toBe(true);
    expect(input.pressed("Digit1")).toBe(false);
  });

  it("clears held keys on blur, hidden and lock loss", () => {
    input = createInput(document.createElement("canvas"));
    key("keydown", "KeyW");
    window.dispatchEvent(new Event("blur"));
    expect(input.held("KeyW")).toBe(false);

    key("keydown", "KeyA");
    document.dispatchEvent(new Event("visibilitychange"));
    // jsdom reports `visible`; the handler clears on any change and lets the
    // next keydown set the key again, which is the safe direction.
    expect(input.held("KeyA")).toBe(false);

    key("keydown", "KeyS");
    document.dispatchEvent(new Event("pointerlockchange"));
    expect(input.held("KeyS")).toBe(false);
  });

  it("ignores mouse movement while the pointer is not locked", () => {
    input = createInput(document.createElement("canvas"));
    document.dispatchEvent(
      new MouseEvent("mousemove", { movementX: 10, movementY: 5 }),
    );
    expect(input.takeLook()).toEqual({ dx: 0, dy: 0 });
  });

  it("stops listening after dispose", () => {
    input = createInput(document.createElement("canvas"));
    input.dispose();
    key("keydown", "KeyW");
    expect(input.held("KeyW")).toBe(false);
  });

  it("does not throw when the browser has no pointer lock", () => {
    const canvas = document.createElement("canvas");
    input = createInput(canvas);
    expect(() => input?.requestLock()).not.toThrow();
    expect(input.locked).toBe(false);
  });

  it("forgets held keys and unconsumed presses on clear", () => {
    input = createInput(document.createElement("canvas"));
    key("keydown", "KeyW");
    key("keydown", "KeyF");
    input.clear();
    expect(input.held("KeyW")).toBe(false);
    expect(input.pressed("KeyF")).toBe(false);
  });

  it("forgets unconsumed presses but keeps held keys on dropPresses", () => {
    input = createInput(document.createElement("canvas"));
    key("keydown", "KeyW");
    key("keydown", "KeyE");
    input.dropPresses();
    expect(input.held("KeyW")).toBe(true);
    expect(input.pressed("KeyE")).toBe(false);
    expect(input.pressed("KeyW")).toBe(false);
    key("keydown", "KeyE");
    expect(input.pressed("KeyE")).toBe(true);
  });
});

describe("typed", () => {
  it("hands over the fresh presses in order, once", () => {
    input = createInput(document.createElement("canvas"));
    key("keydown", "KeyI");
    key("keyup", "KeyI");
    key("keydown", "KeyD");
    key("keydown", "KeyC");
    expect(input.typed()).toEqual(["KeyI", "KeyD", "KeyC"]);
    expect(input.typed()).toEqual([]);
  });

  it("leaves auto-repeat and Ctrl, Cmd and Alt presses out of the log", () => {
    input = createInput(document.createElement("canvas"));
    key("keydown", "KeyD");
    key("keydown", "KeyD", true);
    key("keydown", "KeyD", true);
    window.dispatchEvent(
      new KeyboardEvent("keydown", { code: "KeyC", ctrlKey: true }),
    );
    window.dispatchEvent(
      new KeyboardEvent("keydown", { code: "KeyL", metaKey: true }),
    );
    window.dispatchEvent(
      new KeyboardEvent("keydown", { code: "KeyE", altKey: true }),
    );
    window.dispatchEvent(
      new KeyboardEvent("keydown", { code: "KeyV", shiftKey: true }),
    );
    expect(input.typed()).toEqual(["KeyD", "KeyV"]);
  });

  it("keeps the newest TYPED_CAP presses", () => {
    input = createInput(document.createElement("canvas"));
    const codes = Array.from(
      { length: TYPED_CAP + 4 },
      (_, i) => `Key${String.fromCharCode(65 + i)}`,
    );
    for (const code of codes) key("keydown", code);
    expect(input.typed()).toEqual(codes.slice(4));
  });

  it("forgets the log on clear and on dropPresses", () => {
    input = createInput(document.createElement("canvas"));
    key("keydown", "KeyI");
    input.clear();
    expect(input.typed()).toEqual([]);
    key("keydown", "KeyD");
    input.dropPresses();
    expect(input.typed()).toEqual([]);
  });
});
