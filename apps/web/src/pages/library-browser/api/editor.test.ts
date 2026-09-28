import { describe, expect, test } from "bun:test";
import { parseEditFacts } from "./editor.js";

const controls = {
  exposure: { minimumEv: 0, maximumEv: 1, stepEv: 0.001 },
  whiteBalanceModes: ["as-shot"],
};

const body = (overrides: Record<string, unknown> = {}) => ({
  sourceRevision: "rev-1",
  recipe: null,
  sourceSupport: "supported",
  supportReason: null,
  processingAvailable: true,
  controls,
  ...overrides,
});

const digest = "a".repeat(64);

describe("parseEditFacts", () => {
  test("reads a supported Original source with no reason and no proxy", () => {
    const facts = parseEditFacts(body(), "photo-1");
    expect(facts?.sourceSupport).toBe("supported");
    expect(facts?.supportReason).toBe("");
    expect(facts?.editSource).toBe("original");
    expect(facts?.editSourceProxyId).toBeNull();
    expect(facts?.sourceRevision).toBe("rev-1");
  });

  test("accepts the closed retryable reasons only with an unavailable source", () => {
    for (const supportReason of [
      "read-pending",
      "resource-unavailable",
      "original-missing",
      "original-unreadable",
    ] as const) {
      const facts = parseEditFacts(
        body({
          sourceRevision: null,
          sourceSupport: "unavailable",
          supportReason,
        }),
        "photo-1",
      );
      expect(facts?.supportReason).toBe(supportReason);
      expect(facts?.sourceRevision).toBeNull();
    }
  });

  test("refuses a reason the closed set does not name", () => {
    expect(
      parseEditFacts(
        body({
          sourceRevision: null,
          sourceSupport: "unavailable",
          supportReason: "reindexing",
        }),
        "photo-1",
      ),
    ).toBeUndefined();
  });

  test("refuses a reason beside a supported or unsupported source", () => {
    expect(
      parseEditFacts(body({ supportReason: "read-pending" }), "photo-1"),
    ).toBeUndefined();
    expect(
      parseEditFacts(
        body({
          sourceSupport: "unsupported",
          supportReason: "original-missing",
        }),
        "photo-1",
      ),
    ).toBeUndefined();
  });

  test("refuses a revision beside an unavailable source and its absence beside a supported one", () => {
    expect(
      parseEditFacts(
        body({ sourceSupport: "unavailable", supportReason: "read-pending" }),
        "photo-1",
      ),
    ).toBeUndefined();
    expect(
      parseEditFacts(body({ sourceRevision: null }), "photo-1"),
    ).toBeUndefined();
  });

  test("reads a proxy edit source with its identity digest", () => {
    const facts = parseEditFacts(
      body({ editSource: "development-proxy", editSourceProxyId: digest }),
      "photo-1",
    );
    expect(facts?.editSource).toBe("development-proxy");
    expect(facts?.editSourceProxyId).toBe(digest);
  });

  test("refuses a proxy edit source without a well-formed identity digest", () => {
    expect(
      parseEditFacts(
        body({ editSource: "development-proxy", editSourceProxyId: "proxy-7" }),
        "photo-1",
      ),
    ).toBeUndefined();
    expect(
      parseEditFacts(body({ editSource: "development-proxy" }), "photo-1"),
    ).toBeUndefined();
  });

  test("refuses an unknown edit source kind or an identity beside the Original File", () => {
    expect(
      parseEditFacts(body({ editSource: "thumbnail" }), "photo-1"),
    ).toBeUndefined();
    expect(
      parseEditFacts(
        body({ editSource: "original", editSourceProxyId: digest }),
        "photo-1",
      ),
    ).toBeUndefined();
    // An older server simply omits the field; absence is the Original File.
    expect(factsOf(body({ editSource: undefined }))).toBe("original");
  });
});

const factsOf = (value: Record<string, unknown>): string | undefined =>
  parseEditFacts(value, "photo-1")?.editSource;
