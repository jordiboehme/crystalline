import { afterEach, describe, expect, it } from "vitest";

import { createInput, type Input } from "./input";

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
