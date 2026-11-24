import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { PersonName } from "./components/PersonName";
import { displayFrom, personLabel, personLabelWithLogin } from "./people";

describe("personLabel", () => {
  it("shows the display name when the server sent one", () => {
    expect(personLabel("jordi", "Jordi Böhme")).toBe("Jordi Böhme");
  });

  it("falls back to the login when there is none or it is the login", () => {
    expect(personLabel("jordi", null)).toBe("jordi");
    expect(personLabel("jordi", undefined)).toBe("jordi");
    expect(personLabel("jordi", "  ")).toBe("jordi");
    expect(personLabel("jordi", "jordi")).toBe("jordi");
  });
});

describe("personLabelWithLogin", () => {
  it("adds the login to a display name and leaves a bare login alone", () => {
    expect(personLabelWithLogin("jordi", "Jordi Böhme")).toBe(
      "Jordi Böhme (jordi)",
    );
    expect(personLabelWithLogin("claude", null)).toBe("claude");
  });
});

describe("displayFrom", () => {
  it("reads a login out of a map, and nothing out of none", () => {
    expect(displayFrom({ ada: "Ada Lovelace" }, "ada")).toBe("Ada Lovelace");
    expect(displayFrom({ ada: "Ada Lovelace" }, "bob")).toBeNull();
    expect(displayFrom(undefined, "ada")).toBeNull();
  });
});

describe("PersonName", () => {
  it("shows the display name with the login in its tooltip", () => {
    render(<PersonName login="jordi" display="Jordi Böhme" />);
    const name = screen.getByText("Jordi Böhme");
    expect(name).toHaveAttribute("title", "jordi");
  });

  it("shows Display (login) in a list", () => {
    render(<PersonName login="jordi" display="Jordi Böhme" withLogin />);
    expect(screen.getByText("Jordi Böhme (jordi)")).toHaveAttribute(
      "title",
      "jordi",
    );
  });

  it("shows a login with no display name as it is", () => {
    render(<PersonName login="claude" />);
    expect(screen.getByText("claude")).toHaveAttribute("title", "claude");
  });
});
