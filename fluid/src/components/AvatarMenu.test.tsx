/**
 * The avatar menu's body is a lazy part. When it cannot be loaded, the menu
 * says so as an item a keyboard reaches, and choosing it reloads the page.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

import { AvatarMenu } from "./AvatarMenu";
import { Tooltips } from "./primitives";

vi.mock("./AvatarMenuBody", () => {
  throw new Error("the chunk did not arrive");
});

vi.mock("../auth/AuthContext", () => ({
  useAuth: () => ({
    user: { display: "Ada Lovelace", name: "ada" },
    capabilities: { updateAvailable: null, serverVersion: "0.34.0" },
  }),
}));

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the avatar menu without its body", () => {
  it("offers a reload a keyboard can reach", async () => {
    const reload = vi.fn();
    vi.stubGlobal("location", { ...window.location, reload });
    render(
      <Tooltips>
        <AvatarMenu onShortcuts={() => undefined} />
      </Tooltips>,
    );
    const trigger = screen.getByRole("button", { name: "Ada Lovelace" });
    // A preload that fails is quiet: no unhandled rejection, no message yet.
    trigger.focus();
    const user = userEvent.setup();
    await user.keyboard("{Enter}");
    const item = await screen.findByRole("menuitem", {
      name: "Could not load the menu. Reload",
    });
    await user.keyboard("{ArrowDown}");
    expect(item).toHaveFocus();
    await user.keyboard("{Enter}");
    expect(reload).toHaveBeenCalledOnce();
  });
});
