import { describe, expect, it } from "vitest";

import type { EngramDetail, EngramReference } from "../../api/engram";
import type { GraphNeighborhood, GraphNode } from "../../api/graph";
import type { InboundRefHit, InboundRefPage } from "../../api/inbound";
import { placeKeyOf } from "../paths";
import { HATCH_CAP, type PlaceReference } from "../world/types";
import { placeFromDetail, type PlaceSources } from "./place";

function ref(
  relType: string | null,
  domain: string | null,
  target: string,
  resolved: boolean,
): EngramReference {
  return { line: null, relType, target: { domain, target }, resolved };
}

const DETAIL: EngramDetail = {
  domain: "eng",
  permalink: "hub",
  title: "The Hub",
  url: "crystalline://eng/hub",
  webUrl: null,
  path: "hub.md",
  content: "# The Hub\n\n## Scope\n\nEverything.\n",
  checksum: "abc",
  frontmatter: {
    type: "decision",
    status: "stable",
    tags: ["reactor", "navigation"],
    salience: 6,
    validFrom: "2026-01-01",
    validTo: "2027-01-01",
    staleAfter: null,
    verified: [],
    generatedBy: null,
    generatedModel: null,
  },
  observations: [
    {
      line: 3,
      category: "fact",
      content: "The reactor runs hot.",
      tags: [],
      context: null,
    },
    {
      line: 4,
      category: null,
      content: "Nobody knows why.",
      tags: ["mystery"],
      context: "ops",
    },
  ],
  relations: [
    ref("depends_on", null, "Reactor Core", true),
    ref("supersedes", null, "old-bridge", true),
    ref("relates_to", null, "Nowhere", false),
  ],
  links: [ref(null, "ops", "Runbook", true), ref(null, "e", "Deep Note", true)],
  inboundCount: 7,
  inboundRefs: [],
  similar: [],
  guidance: null,
  draft: false,
  draftOwner: null,
  localChange: null,
};

function node(
  id: number,
  domain: string,
  permalink: string,
  title: string,
): GraphNode {
  return { id, domain, permalink, title, status: null, type: null };
}

const GRAPH: GraphNeighborhood = {
  nodes: [
    node(0, "eng", "hub", "The Hub"),
    node(1, "eng", "reactor-core", "Reactor Core"),
    node(2, "ops", "runbook", "Runbook"),
    node(3, "e", "notes/deep note", "Deep Note"),
  ],
  edges: [],
  truncated: false,
  hidden: 0,
};

const DOMAINS = ["eng", "ops", "e"] as const;

function hit(domain: string, permalink: string, rel: string): InboundRefHit {
  return {
    domain,
    permalink,
    title: `Title ${permalink}`,
    path: `${permalink}.md`,
    status: null,
    rel,
  };
}

// Thirty distinct sources, alternating between two domains and written in an
// order that is neither sorted nor reverse sorted.
const HITS: InboundRefHit[] = Array.from({ length: 30 }, (_, i) => {
  const n = (i * 7) % 30;
  return hit(
    i % 2 === 0 ? "eng" : "alpha",
    `src-${String(n).padStart(2, "0")}`,
    i % 3 === 0 ? "links_to" : "depends_on",
  );
});

function page(hits: InboundRefHit[], total: number): InboundRefPage {
  return {
    total,
    page: 1,
    limit: 24,
    count: hits.length,
    types: [],
    hits,
  };
}

const SOURCES: PlaceSources = {
  detail: DETAIL,
  graph: GRAPH,
  domains: DOMAINS,
  inbound: page(HITS, 300),
  targetSalience: new Map([[placeKeyOf("eng", "reactor-core"), 8]]),
};

function byTarget(list: readonly PlaceReference[]) {
  return (text: string) => list.find((r) => r.target.target === text);
}

describe("placeFromDetail relations", () => {
  const place = placeFromDetail(SOURCES);
  const relation = byTarget(place.relations);

  it("locates a resolved relation through the graph", () => {
    expect(relation("Reactor Core")).toEqual({
      relType: "depends_on",
      target: { domain: null, target: "Reactor Core" },
      resolved: true,
      address: { domain: "eng", permalink: "reactor-core" },
      targetTitle: "Reactor Core",
      targetSalience: 8,
      // GRAPH's node carries type: null.
      targetType: null,
    });
  });

  it("keeps a resolved relation the graph did not locate resolved but without an address", () => {
    expect(relation("old-bridge")).toEqual({
      relType: "supersedes",
      target: { domain: null, target: "old-bridge" },
      resolved: true,
      address: null,
      targetTitle: null,
      targetSalience: null,
    });
  });

  it("marks an unresolved relation unresolved", () => {
    expect(relation("Nowhere")).toEqual({
      relType: "relates_to",
      target: { domain: null, target: "Nowhere" },
      resolved: false,
      address: null,
      targetTitle: null,
      targetSalience: null,
    });
  });

  it("keeps the relations in document order", () => {
    expect(place.relations.map((r) => r.target.target)).toEqual([
      "Reactor Core",
      "old-bridge",
      "Nowhere",
    ]);
  });
});

describe("placeFromDetail links", () => {
  const place = placeFromDetail(SOURCES);
  const link = byTarget(place.links);

  it("locates a cross-domain wikilink and keeps its domain prefix", () => {
    expect(link("Runbook")).toEqual({
      relType: null,
      target: { domain: "ops", target: "Runbook" },
      resolved: true,
      address: { domain: "ops", permalink: "runbook" },
      targetTitle: "Runbook",
      // The salience map does not know it.
      targetSalience: null,
      // GRAPH's node carries type: null.
      targetType: null,
    });
  });

  it("reads back an address in a domain named e with a spaced, nested permalink", () => {
    expect(link("Deep Note")?.address).toEqual({
      domain: "e",
      permalink: "notes/deep note",
    });
    expect(link("Deep Note")?.targetTitle).toBe("Deep Note");
  });
});

const TYPED_GRAPH: GraphNeighborhood = {
  ...GRAPH,
  nodes: [
    ...GRAPH.nodes.map((n) =>
      n.permalink === "reactor-core"
        ? { ...n, type: "runbook" }
        : n.permalink === "runbook"
          ? { ...n, type: "guide" }
          : n,
    ),
    {
      id: 9,
      domain: "eng",
      permalink: "src-00",
      title: "Source",
      status: null,
      type: "reference",
    },
  ],
};

describe("placeFromDetail neighbour types (2.6f C10)", () => {
  const place = placeFromDetail({ ...SOURCES, graph: TYPED_GRAPH });

  it("gives a located way its target's type, null where the node carries none", () => {
    // Mutation caught: the type left off, read from the wrong node, or a
    // null type dropped as if unknown.
    const relation = byTarget(place.relations);
    const link = byTarget(place.links);
    expect(relation("Reactor Core")?.targetType).toBe("runbook");
    expect(link("Runbook")?.targetType).toBe("guide");
    expect(link("Deep Note")).toHaveProperty("targetType", null);
  });

  it("leaves the type absent on a way that leads nowhere", () => {
    // Mutation caught: a sealed or unresolved way given a type.
    const relation = byTarget(place.relations);
    expect(relation("old-bridge")).not.toHaveProperty("targetType");
    expect(relation("Nowhere")).not.toHaveProperty("targetType");
  });

  it("gives a hatch its source's type when the graph holds it, and none when not", () => {
    // Mutation caught: every hatch typed null (unknown read as known), or
    // the lookup keyed by title.
    const known = place.inbound.find((h) => h.address.permalink === "src-00");
    expect(known?.type).toBe("reference");
    const unknown = place.inbound.filter(
      (h) => h.address.permalink !== "src-00",
    );
    expect(unknown.length).toBeGreaterThan(0);
    for (const h of unknown) expect(h).not.toHaveProperty("type");
  });
});

describe("placeFromDetail inbound", () => {
  it("caps the hatches, sorts them by address and keeps the true total", () => {
    const place = placeFromDetail(SOURCES);
    expect(place.inbound).toHaveLength(HATCH_CAP);
    const keys = place.inbound.map((h) =>
      placeKeyOf(h.address.domain, h.address.permalink),
    );
    const everyKey = HITS.map((h) => placeKeyOf(h.domain, h.permalink)).sort();
    expect(keys).toEqual(everyKey.slice(0, HATCH_CAP));
    expect(place.inboundTotal).toBe(300);
    const first = place.inbound[0];
    expect(first).toEqual({
      address: { domain: "alpha", permalink: "src-01" },
      title: "Title src-01",
      relType: HITS.find(
        (h) => h.domain === "alpha" && h.permalink === "src-01",
      )?.rel,
    });
  });

  it("sorts by domain first, then permalink", () => {
    const place = placeFromDetail({
      ...SOURCES,
      inbound: page(
        [hit("b", "a", "x"), hit("a", "z", "x"), hit("a", "b", "x")],
        3,
      ),
    });
    expect(place.inbound.map((h) => h.address)).toEqual([
      { domain: "a", permalink: "b" },
      { domain: "a", permalink: "z" },
      { domain: "b", permalink: "a" },
    ]);
  });

  it("deduplicates by address and keeps the smallest relation", () => {
    const place = placeFromDetail({
      ...SOURCES,
      inbound: page(
        [
          hit("eng", "a", "supersedes"),
          hit("eng", "b", "links_to"),
          hit("eng", "a", "links_to"),
        ],
        3,
      ),
    });
    expect(place.inbound.map((h) => [h.address.permalink, h.relType])).toEqual([
      ["a", "links_to"],
      ["b", "links_to"],
    ]);
    expect(place.inboundTotal).toBe(3);
  });

  it("picks the same relation for one address whatever order the page lists it in", () => {
    const relsFor = (hits: InboundRefHit[]) =>
      placeFromDetail({ ...SOURCES, inbound: page(hits, 2) }).inbound.map(
        (h) => h.relType,
      );
    const relates = hit("eng", "a", "relates_to");
    const depends = hit("eng", "a", "depends_on");
    expect(relsFor([relates, depends])).toEqual(["depends_on"]);
    expect(relsFor([depends, relates])).toEqual(["depends_on"]);
  });

  it("gives no hatches and the detail's count without an inbound page", () => {
    const place = placeFromDetail({ ...SOURCES, inbound: null });
    expect(place.inbound).toEqual([]);
    expect(place.inboundTotal).toBe(DETAIL.inboundCount);
  });
});

describe("placeFromDetail without a graph", () => {
  it("gives every resolved reference no address", () => {
    const place = placeFromDetail({ ...SOURCES, graph: null });
    for (const r of [...place.relations, ...place.links]) {
      expect(r.address).toBeNull();
      expect(r.targetTitle).toBeNull();
      expect(r.targetSalience).toBeNull();
    }
    expect(place.relations.map((r) => r.resolved)).toEqual([true, true, false]);
    expect(place.links.map((r) => r.resolved)).toEqual([true, true]);
  });
});

describe("placeFromDetail without a domain listing", () => {
  it("still locates bare and prefixed links when the listing is unknown", () => {
    const place = placeFromDetail({ ...SOURCES, domains: undefined });
    const link = byTarget(place.links);
    const relation = byTarget(place.relations);
    expect(relation("Reactor Core")?.address).toEqual({
      domain: "eng",
      permalink: "reactor-core",
    });
    expect(link("Runbook")?.address).toEqual({
      domain: "ops",
      permalink: "runbook",
    });
  });
});

describe("placeFromDetail fields", () => {
  const place = placeFromDetail(SOURCES);

  it("keeps the observations' category and content in document order", () => {
    expect(place.observations).toEqual([
      { category: "fact", content: "The reactor runs hot." },
      { category: null, content: "Nobody knows why." },
    ]);
  });

  it("reads the frontmatter fields", () => {
    expect(place).toMatchObject({
      domain: "eng",
      permalink: "hub",
      title: "The Hub",
      type: "decision",
      status: "stable",
      salience: 6,
      validFrom: "2026-01-01",
      validTo: "2027-01-01",
      tags: ["reactor", "navigation"],
      content: DETAIL.content,
    });
  });
});

describe("placeFromDetail determinism", () => {
  it("does not depend on the order of graph nodes or inbound hits", () => {
    const shuffled: PlaceSources = {
      ...SOURCES,
      graph: { ...GRAPH, nodes: [...GRAPH.nodes].reverse() },
      inbound: page([...HITS.slice(11), ...HITS.slice(0, 11)].reverse(), 300),
    };
    expect(JSON.stringify(placeFromDetail(shuffled))).toBe(
      JSON.stringify(placeFromDetail(SOURCES)),
    );
  });
});
