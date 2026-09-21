/**
 * The names a download leaves on disk.
 *
 * A name is the only thing a reader keeps once the file is out of the browser,
 * so what is pinned here is that it says which document the picture came from,
 * that two pictures in one document never collide, and that the same picture
 * comes back under the same name on every visit.
 */

import { describe, expect, it } from "vitest";

import { diagramFileName, fnv1a32, imageFileName } from "./downloads";

describe("the download names", () => {
  it("hashes with FNV-1a, the published vectors included", () => {
    // The two vectors every FNV-1a implementation is checked against.
    expect(fnv1a32("")).toBe("811c9dc5");
    expect(fnv1a32("a")).toBe("e40c292c");
    expect(fnv1a32("graph TD; A-->B;")).toHaveLength(8);
    expect(fnv1a32("graph TD; A-->B;")).not.toBe(fnv1a32("graph TD; A-->C;"));
  });

  it("names a diagram by the document, the word and four hash characters", () => {
    const hash = fnv1a32("graph TD; A-->B;").slice(0, 4);
    expect(diagramFileName("alpha", "graph TD; A-->B;", "mmd")).toBe(
      `alpha-diagram-${hash}.mmd`,
    );
    expect(diagramFileName("alpha", "graph TD; A-->B;", "svg")).toBe(
      `alpha-diagram-${hash}.svg`,
    );
    // Two diagrams in one document never collide; the same one is stable.
    expect(diagramFileName("alpha", "graph TD; A-->C;", "mmd")).not.toBe(
      diagramFileName("alpha", "graph TD; A-->B;", "mmd"),
    );
    expect(diagramFileName(undefined, "graph TD; A-->B;", "mmd")).toBe(
      `diagram-${hash}.mmd`,
    );
  });

  it("names an image by the last segment of what the author wrote", () => {
    expect(imageFileName("assets/map.png")).toBe("map.png");
    expect(imageFileName("assets/deep/map%20of%20town.png#align=left")).toBe(
      "map of town.png",
    );
    expect(imageFileName("https://example.org/pictures/x.jpg?size=large")).toBe(
      "x.jpg",
    );
    expect(imageFileName("")).toBe("image");
  });
});
