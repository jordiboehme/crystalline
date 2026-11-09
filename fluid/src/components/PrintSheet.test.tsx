import { act, render, waitFor } from "@testing-library/react";
import { encode } from "uqr";
import { afterEach, describe, expect, it, vi } from "vitest";

import { qrPath } from "../printSheet";
import { PrintPageStyle, PrintTitleBlock } from "./PrintSheet";

afterEach(() => {
  vi.useRealTimers();
});

function page() {
  return render(
    <main>
      <PrintPageStyle
        source="eng"
        title="Alpha"
        address="crystalline://eng/alpha"
      />
      <PrintTitleBlock
        trail={["eng"]}
        title="Alpha"
        address="crystalline://eng/alpha"
        webAddress="https://fluid.example/d/eng/e/alpha"
      />
      <img alt="far down" loading="lazy" src="data:," />
    </main>,
  );
}

describe("the printed page's sheet", () => {
  it("reads the print date again as printing starts", () => {
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2026, 9, 16, 12));
    const { container } = page();
    const block = container.querySelector("header.print-title");
    const style = container.querySelector("style[data-print-page]");
    const opened = new Date(2026, 9, 16, 12).toLocaleDateString(undefined, {
      day: "numeric",
      month: "short",
      year: "numeric",
    });
    expect(block).toHaveTextContent(`Printed ${opened}`);

    // The tab stays open overnight and is printed the next day.
    vi.setSystemTime(new Date(2026, 9, 17, 9));
    const printed = new Date(2026, 9, 17, 9).toLocaleDateString(undefined, {
      day: "numeric",
      month: "short",
      year: "numeric",
    });
    act(() => {
      window.dispatchEvent(new Event("beforeprint"));
    });
    expect(block).toHaveTextContent(`Printed ${printed}`);
    expect(style?.textContent).toContain(`Printed ${printed}`);
  });

  it("loads the page's lazy pictures for the printer, and lets them be lazy again after", () => {
    const { container } = page();
    const image = container.querySelector("img");
    expect(image?.getAttribute("loading")).toBe("lazy");
    act(() => {
      window.dispatchEvent(new Event("beforeprint"));
    });
    expect(image?.getAttribute("loading")).toBe("eager");
    act(() => {
      window.dispatchEvent(new Event("afterprint"));
    });
    expect(image?.getAttribute("loading")).toBe("lazy");
  });

  it("stamps the page's web address as a QR code on paper only", async () => {
    const { container } = page();
    const svg = await waitFor(() => {
      const found = container.querySelector<SVGElement>("svg[data-qr-text]");
      expect(found).not.toBeNull();
      return found as SVGElement;
    });
    // On paper only: inside the hidden print block, never on screen.
    expect(svg.closest("[hidden][data-print]")).not.toBeNull();
    expect(svg.closest("header.print-title")).not.toBeNull();
    // The web address, not the crystalline:// name, at level M with a
    // four-module quiet zone: the very modules the encoder gives for it.
    const address = "https://fluid.example/d/eng/e/alpha";
    expect(svg.getAttribute("data-qr-text")).toBe(address);
    const expected = encode(address, { ecc: "M", border: 4 });
    expect(Number(svg.getAttribute("data-qr-size"))).toBe(expected.size);
    expect(svg.getAttribute("viewBox")).toBe(
      `0 0 ${String(expected.size)} ${String(expected.size)}`,
    );
    expect(svg.querySelector("path")?.getAttribute("d")).toBe(
      qrPath(expected.data),
    );
    // The quiet zone: the first four rows and columns are light.
    for (const row of expected.data.slice(0, 4)) {
      expect(row.every((dark) => !dark)).toBe(true);
    }
    expect(expected.data.every((row) => row.slice(0, 4).every((d) => !d))).toBe(
      true,
    );
    expect(svg.getAttribute("width")).toBe("22mm");
    expect(svg.closest("figure")).toHaveTextContent("Open in Fluid");
  });

  it("makes the QR code and its caption a link to the page, for a PDF reader", async () => {
    const { container } = page();
    const svg = await waitFor(() => {
      const found = container.querySelector<SVGElement>("svg[data-qr-text]");
      expect(found).not.toBeNull();
      return found as SVGElement;
    });
    const link = svg.closest("a");
    expect(link).not.toBeNull();
    expect(link?.getAttribute("href")).toBe(
      "https://fluid.example/d/eng/e/alpha",
    );
    expect(link).toHaveTextContent("Open in Fluid");
  });
});
