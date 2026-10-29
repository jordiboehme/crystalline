import { describe, expect, it } from "vitest";

import {
  atLeast,
  mayEditPage,
  readReflectionDomains,
  readReflectionStatus,
} from "./reflectionDomains";

describe("reflection status", () => {
  it("reads the contract's shape", () => {
    expect(
      readReflectionStatus(
        {
          domain: "eng",
          enabled: true,
          state: "elsewhere",
          home: "elsewhere",
          home_url: "https://team.example.com",
          right: "read",
          engrams_visible: true,
          member: true,
          governance: "github",
          private: true,
          pending_engrams: [],
        },
        "eng",
      ),
    ).toEqual({
      domain: "eng",
      state: "elsewhere",
      home: "elsewhere",
      homeUrl: "https://team.example.com",
      hostLabel: null,
      right: "read",
      engramsVisible: true,
      member: true,
      governance: "github",
      private: true,
      pendingEngrams: [],
    });
  });

  it("reads anything it does not know as off and none, which draws nothing", () => {
    expect(
      readReflectionStatus({ state: "sideways", right: "admin" }, "eng"),
    ).toMatchObject({
      domain: "eng",
      state: "off",
      right: "none",
      engramsVisible: false,
      member: false,
    });
    // The first contract's word: no longer a right, so it draws nothing.
    expect(
      readReflectionStatus({ state: "on", right: "comment" }, "eng").right,
    ).toBe("none");
  });

  it("reads a homeless Reflection as homeless, never as off, and a missing home as null", () => {
    const status = readReflectionStatus(
      {
        domain: "eng",
        enabled: true,
        state: "homeless",
        home: null,
        home_url: null,
        right: "curate",
        engrams_visible: true,
        member: false,
        pending_engrams: [],
      },
      "eng",
    );
    expect(status.state).toBe("homeless");
    expect(status.home).toBeNull();
  });

  it("reads a Reflection another instance serves, with the host's label", () => {
    const status = readReflectionStatus(
      {
        domain: "eng",
        enabled: true,
        state: "hosted_elsewhere",
        home: null,
        home_url: null,
        host_label: "office-mac",
        right: "curate",
        engrams_visible: true,
        member: false,
        pending_engrams: [],
      },
      "eng",
    );
    expect(status.state).toBe("hosted_elsewhere");
    expect(status.hostLabel).toBe("office-mac");
    expect(
      readReflectionDomains({
        domains: [{ domain: "eng", state: "hosted_elsewhere", right: "read" }],
      })[0]?.state,
    ).toBe("hosted_elsewhere");
  });

  it("orders the rights", () => {
    expect(atLeast("edit", "read")).toBe(true);
    expect(atLeast("read", "edit")).toBe(false);
    expect(atLeast("curate", "curate")).toBe(true);
  });

  it("leaves PACT.md to curators", () => {
    expect(mayEditPage("edit", "guides/a.md")).toBe(true);
    expect(mayEditPage("edit", "PACT.md")).toBe(false);
    expect(mayEditPage("curate", "PACT.md")).toBe(true);
    expect(mayEditPage("read", "guides/a.md")).toBe(false);
  });
});

describe("reflection domains", () => {
  it("keeps the rows a caller reaches and drops the rest", () => {
    expect(
      readReflectionDomains({
        domains: [
          {
            domain: "eng",
            state: "on",
            right: "edit",
            engrams_visible: false,
            home: "here",
            home_url: null,
            open_threads: 3,
          },
          {
            domain: "ops",
            state: "on",
            right: "none",
            engrams_visible: true,
            home: "here",
            open_threads: 0,
          },
          {
            domain: "lab",
            state: "pending",
            right: "read",
            engrams_visible: false,
            home: null,
            home_url: null,
            open_threads: 0,
          },
          { right: "edit" },
        ],
      }),
    ).toEqual([
      {
        domain: "eng",
        state: "on",
        right: "edit",
        engramsVisible: false,
        home: "here",
        homeUrl: null,
        hostLabel: null,
        openThreads: 3,
      },
      {
        domain: "lab",
        state: "pending",
        right: "read",
        engramsVisible: false,
        home: null,
        homeUrl: null,
        hostLabel: null,
        openThreads: 0,
      },
    ]);
  });
});

describe("reflection reads, more", () => {
  it("reads a home of here with a null host label", () => {
    const status = readReflectionStatus(
      {
        domain: "eng",
        state: "on",
        home: "here",
        home_url: null,
        host_label: null,
        right: "edit",
      },
      "eng",
    );
    expect(status).toMatchObject({
      home: "here",
      homeUrl: null,
      hostLabel: null,
    });
  });

  it("reads the host label of a listing row, and a null one", () => {
    const rows = readReflectionDomains({
      domains: [
        {
          domain: "a",
          state: "hosted_elsewhere",
          right: "read",
          host_label: "office-mac",
        },
        { domain: "b", state: "on", right: "read", host_label: null },
      ],
    });
    expect(rows.map((r) => r.hostLabel)).toEqual(["office-mac", null]);
  });

  it("reads an unknown state of a listing row as on: the server lists a row only when it is not off", () => {
    expect(
      readReflectionDomains({
        domains: [{ domain: "a", state: "sideways", right: "read" }],
      })[0]?.state,
    ).toBe("on");
  });
});
