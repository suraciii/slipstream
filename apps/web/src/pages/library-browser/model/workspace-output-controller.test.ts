import { describe, expect, test } from "bun:test";
import {
  createWorkspaceOutputController,
  imageOutputView,
  parseXmpArtifact,
  type OutputFacts,
} from "./workspace-output-controller.js";
import type { ExportInspection } from "./photo-export.js";
import type { BrowserFetch } from "./access-session.js";

const facts: OutputFacts = {
  recipeVersion: "new",
  sourceRevision: "source",
  recipeSourceRevision: "source",
  saving: false,
  dirty: false,
  conflict: false,
  canRender: true,
  canRenderFilm: false,
};
const retained: ExportInspection = {
  exportId: "old",
  target: "development-tiff",
  state: "succeeded",
  failureReason: "",
  recipeVersion: "old",
  sourceRevision: "source",
  createdAt: "2026-01-01T00:00:00Z",
  artifact: {
    exportId: "old",
    target: "development-tiff",
    stage: "develop",
    contentType: "image/tiff",
    width: 6000,
    height: 4000,
    profileIdentity: "profile",
    filename: "old.tiff",
    orientation: "landscape",
    sampleFormat: "float",
    colorSpace: "ProPhoto",
    iccEmbedded: true,
    byteLength: 123,
    sha256: "a".repeat(64),
    expiresAt: "2999-01-01T00:00:00Z",
  },
};
const xmpRecord = {
  exportId: "xmp",
  photoId: "photo",
  target: "edit-state-xmp",
  state: "succeeded",
  recipeVersion: "new",
  sourceRevision: "source",
  createdAt: "2026-01-01T00:00:00Z",
  expiresAt: "2999-01-01T00:00:00Z",
  artifact: {
    filename: "edit.xmp",
    contentType: "application/rdf+xml",
    byteLength: 42,
    sha256: "a".repeat(64),
  },
};

describe("retained workspace results", () => {
  test.each(["queued", "running", "failed", "cancelled"] as const)(
    "keeps download and old provenance during a newer %s task",
    (state) => {
      const active: ExportInspection = {
        ...retained,
        exportId: "new-task",
        state,
        recipeVersion: "new",
        artifact: null,
      };
      const view = imageOutputView(
        { state, note: "attempt", active, retained },
        "development-tiff",
        facts,
        false,
      );
      expect(view.canDownload).toBe(true);
      expect(view.isStale).toBe(true);
      expect(view.artifact?.exportId).toBe("old");
    },
  );
  test("expiry disables old downloads without claiming they match the latest edit", () => {
    const old = {
      ...retained,
      artifact: { ...retained.artifact!, expiresAt: "2000-01-01T00:00:00Z" },
    };
    const view = imageOutputView(
      { state: "succeeded", note: "ready", active: old, retained: old },
      "development-tiff",
      facts,
      false,
    );
    expect(view.canDownload).toBe(false);
    expect(view.isStale).toBe(true);
  });
  test("unconfirmed edits block new TIFF while retaining old download", () => {
    const view = imageOutputView(
      { state: "succeeded", note: "ready", active: retained, retained },
      "development-tiff",
      { ...facts, saving: true, dirty: true },
      false,
    );
    expect(view.canSubmit).toBe(false);
    expect(view.canDownload).toBe(true);
  });
  test("reopening hydrates summaries and preserves the last success after a failed attempt", async () => {
    const failed = {
      ...retained,
      exportId: "failed",
      state: "failed",
      createdAt: "2026-01-02T00:00:00Z",
      artifact: null,
      failureReason: "source_unavailable",
    };
    const fetcher: BrowserFetch = (path) => {
      if (typeof path !== "string") throw new Error("Expected endpoint path");
      const data = path.endsWith("/edit-state-exports")
        ? { exports: [] }
        : path.endsWith("/exports")
          ? { exports: [{ exportId: "failed" }, { exportId: "old" }] }
          : path.endsWith("/failed")
            ? failed
            : retained;
      return Promise.resolve(new Response(JSON.stringify(data)));
    };
    const owner = createWorkspaceOutputController(fetcher, {
      facts: () => facts,
      owns: () => true,
      render: () => {},
      settle: () => Promise.resolve(true),
      acquire: () => {},
      release: () => {},
    });
    owner.open("photo");
    await owner.refresh("photo");
    const view = owner.view("photo").tiff;
    expect(view.state).toBe("failed");
    expect(view.artifact?.exportId).toBe("old");
    expect(view.createdAt).toBe(retained.createdAt ?? null);
    expect(view.canDownload).toBe(true);
    expect(view.isStale).toBe(true);
    owner.leave();
  });
  test("an unavailable inspection does not remove the previous download", async () => {
    let unavailable = false;
    const fetcher: BrowserFetch = (path) => {
      if (typeof path !== "string") throw new Error("Expected endpoint path");
      if (path.endsWith("/old") && unavailable)
        return Promise.resolve(new Response(null, { status: 503 }));
      const data = path.endsWith("/edit-state-exports")
        ? { exports: [] }
        : path.endsWith("/exports")
          ? { exports: [{ exportId: "old" }] }
          : retained;
      return Promise.resolve(new Response(JSON.stringify(data)));
    };
    const owner = createWorkspaceOutputController(fetcher, {
      facts: () => facts,
      owns: () => true,
      render: () => {},
      settle: () => Promise.resolve(true),
      acquire: () => {},
      release: () => {},
    });
    owner.open("photo");
    await owner.refresh("photo");
    unavailable = true;
    await owner.refresh("photo");
    expect(owner.view("photo").tiff.artifact?.exportId).toBe("old");
    expect(owner.view("photo").tiff.canDownload).toBe(true);
    owner.leave();
  });
  test("a terminal success without an artifact settles without background polling", async () => {
    const previousWindow = Object.getOwnPropertyDescriptor(
      globalThis,
      "window",
    );
    let polls = 0;
    Object.defineProperty(globalThis, "window", {
      configurable: true,
      value: {
        setTimeout: () => {
          polls++;
          return 0;
        },
      },
    });
    const terminal = { ...retained, artifact: null };
    const fetcher: BrowserFetch = (path) =>
      Promise.resolve(
        new Response(
          JSON.stringify(
            typeof path === "string" && path.endsWith("/edit-state-exports")
              ? { exports: [] }
              : typeof path === "string" && path.endsWith("/exports")
                ? { exports: [terminal] }
                : terminal,
          ),
        ),
      );
    const owner = createWorkspaceOutputController(fetcher, {
      facts: () => facts,
      owns: () => true,
      render: () => {},
      settle: () => Promise.resolve(true),
      acquire: () => {},
      release: () => {},
    });
    try {
      owner.open("photo");
      await owner.refresh("photo");
      expect(owner.view("photo").tiff.canSubmit).toBe(true);
      expect(owner.view("photo").tiff.canDownload).toBe(false);
      expect(polls).toBe(0);
    } finally {
      owner.leave();
      if (previousWindow)
        Object.defineProperty(globalThis, "window", previousWindow);
      else Reflect.deleteProperty(globalThis, "window");
    }
  });
  test("a delayed refresh cannot replace a newer task state", async () => {
    let delay = false;
    const pending: Array<(response: Response) => void> = [];
    const failed = { ...retained, state: "failed", artifact: null };
    const fetcher: BrowserFetch = (path) => {
      if (typeof path !== "string") throw new Error("Expected endpoint path");
      if (delay && path.endsWith("/exports")) {
        return new Promise<Response>((resolve) => pending.push(resolve));
      }
      const data = path.endsWith("/edit-state-exports")
        ? { exports: [] }
        : path.endsWith("/exports")
          ? { exports: [retained] }
          : failed;
      return Promise.resolve(new Response(JSON.stringify(data)));
    };
    const owner = createWorkspaceOutputController(fetcher, {
      facts: () => facts,
      owns: () => true,
      render: () => {},
      settle: () => Promise.resolve(true),
      acquire: () => {},
      release: () => {},
    });
    owner.open("photo");
    await owner.refresh("photo");
    delay = true;
    const obsolete = owner.refresh("photo");
    delay = false;
    await owner.refresh("photo");
    pending.forEach((resolve) =>
      resolve(new Response(JSON.stringify({ exports: [] }))),
    );
    await obsolete;
    expect(owner.view("photo").tiff.state).toBe("failed");
    owner.leave();
  });
  test("a newer unhydrated task prevents retrying an older failure", async () => {
    const newest = {
      ...retained,
      exportId: "newest",
      state: "running",
      createdAt: "2026-01-03T00:00:00Z",
      artifact: null,
    };
    const fetcher: BrowserFetch = (path) => {
      if (typeof path !== "string") throw new Error("Expected endpoint path");
      if (path.endsWith("/newest"))
        return Promise.resolve(new Response(null, { status: 503 }));
      const data = path.endsWith("/edit-state-exports")
        ? { exports: [] }
        : path.endsWith("/exports")
          ? { exports: [newest, retained] }
          : retained;
      return Promise.resolve(new Response(JSON.stringify(data)));
    };
    const owner = createWorkspaceOutputController(fetcher, {
      facts: () => facts,
      owns: () => true,
      render: () => {},
      settle: () => Promise.resolve(true),
      acquire: () => {},
      release: () => {},
    });
    const previousWindow = Object.getOwnPropertyDescriptor(
      globalThis,
      "window",
    );
    let pollsScheduled = 0;
    Object.defineProperty(globalThis, "window", {
      configurable: true,
      value: {
        setTimeout: () => {
          pollsScheduled++;
          return 0;
        },
      },
    });
    try {
      owner.open("photo");
      await owner.refresh("photo");
      const view = owner.view("photo").tiff;
      expect(view.state).toBe("running");
      expect(view.canRetry).toBe(false);
      expect(view.canDownload).toBe(true);
      expect(view.artifact?.exportId).toBe("old");
      expect(pollsScheduled).toBe(1);
    } finally {
      owner.leave();
      if (previousWindow)
        Object.defineProperty(globalThis, "window", previousWindow);
      else Reflect.deleteProperty(globalThis, "window");
    }
  });
});

describe("XMP evidence and reconciliation", () => {
  test("reopening an expired XMP retains its evidence and offers a new export", async () => {
    const record = { ...xmpRecord, expiresAt: "2020-01-01T00:00:00Z" };
    const fetcher: BrowserFetch = (path) =>
      Promise.resolve(
        new Response(
          JSON.stringify({
            exports:
              typeof path === "string" && path.endsWith("/edit-state-exports")
                ? [record]
                : [],
          }),
        ),
      );
    const owner = createWorkspaceOutputController(fetcher, {
      facts: () => facts,
      owns: () => true,
      render: () => {},
      settle: () => Promise.resolve(true),
      acquire: () => {},
      release: () => {},
    });
    owner.open("photo");
    await owner.refresh("photo");
    const view = owner.view("photo").xmp;
    expect(view.artifact?.exportId).toBe(record.exportId);
    expect(view.canDownload).toBe(false);
    expect(view.canSubmit).toBe(true);
    expect(view.note).toContain("expired");
    owner.leave();
  });
  test("refuses another Photo and an unsupported payload or size", () => {
    expect(parseXmpArtifact(xmpRecord, "other")).toBeUndefined();
    expect(
      parseXmpArtifact(
        {
          ...xmpRecord,
          artifact: { ...xmpRecord.artifact, byteLength: 65537 },
        },
        "photo",
      ),
    ).toBeUndefined();
    expect(
      parseXmpArtifact(
        { ...xmpRecord, artifact: { ...xmpRecord.artifact, sha256: "bad" } },
        "photo",
      ),
    ).toBeUndefined();
  });
  test("exports retained confirmed state without an available Original or engine", async () => {
    const bodies: unknown[] = [];
    const fetcher: BrowserFetch = (_path, init) => {
      if (typeof init?.body !== "string") throw new Error("Expected JSON body");
      bodies.push(JSON.parse(init.body));
      return Promise.resolve(
        new Response(JSON.stringify(xmpRecord), { status: 201 }),
      );
    };
    const owner = createWorkspaceOutputController(fetcher, {
      facts: () => ({ ...facts, sourceRevision: null, canRender: false }),
      owns: () => true,
      render: () => {},
      settle: () => Promise.resolve(true),
      acquire: () => {},
      release: () => {},
    });
    expect(owner.view("photo").xmp.canSubmit).toBe(true);
    expect(owner.view("photo").tiff.canSubmit).toBe(false);
    await owner.submit("photo");
    expect(bodies).toEqual([
      expect.objectContaining({
        expectedSourceRevision: "source",
        expectedRecipeVersion: "new",
      }),
    ]);
    expect(owner.view("photo").xmp.canDownload).toBe(true);
  });
  test("lost XMP response retries the same identity and blocks later output until resolved", async () => {
    const bodies: unknown[] = [];
    let attempts = 0;
    let released = 0;
    const fetcher: BrowserFetch = (_path, init) => {
      if (typeof init?.body !== "string") throw new Error("Expected JSON body");
      bodies.push(JSON.parse(init.body));
      if (attempts++ === 0) return Promise.reject(new Error("lost response"));
      return Promise.resolve(
        new Response(JSON.stringify(xmpRecord), { status: 200 }),
      );
    };
    const owner = createWorkspaceOutputController(fetcher, {
      facts: () => facts,
      owns: () => true,
      render: () => {},
      settle: () => Promise.resolve(true),
      acquire: () => {},
      release: () => {
        released++;
      },
    });
    await owner.submit("photo");
    expect(owner.view("photo").xmp.state).toBe("outcome-unknown");
    expect(owner.view("photo").tiff.canSubmit).toBe(false);
    await owner.submit("photo");
    expect(bodies[1]).toEqual(bodies[0]);
    expect(owner.view("photo").xmp.state).toBe("succeeded");
    expect(released).toBe(1);
  });
  test.each([
    "processing_unavailable",
    "resource_unavailable",
    "retained_output_full",
  ])(
    "a definitive %s refusal releases the output barrier for another export",
    async (code) => {
      let released = 0;
      let refuse = true;
      const fetcher: BrowserFetch = () =>
        Promise.resolve(
          refuse
            ? new Response(
                JSON.stringify({ error: { code, effect: "none" } }),
                { status: 503 },
              )
            : new Response(JSON.stringify(xmpRecord), { status: 201 }),
        );
      const owner = createWorkspaceOutputController(fetcher, {
        facts: () => facts,
        owns: () => true,
        render: () => {},
        settle: () => Promise.resolve(true),
        acquire: () => {},
        release: () => {
          released++;
        },
      });
      await owner.submit("photo", "development-tiff");
      expect(owner.view("photo").tiff.state).toBe("failed");
      expect(owner.pending("photo")).toBe(false);
      expect(owner.view("photo").xmp.canSubmit).toBe(true);
      refuse = false;
      await owner.submit("photo");
      expect(owner.view("photo").xmp.canDownload).toBe(true);
      expect(released).toBe(2);
    },
  );
  test("failed saving never sends a parameter export", async () => {
    let sent = false;
    const fetcher: BrowserFetch = () => {
      sent = true;
      return Promise.reject(new Error("unexpected export"));
    };
    const owner = createWorkspaceOutputController(fetcher, {
      facts: () => ({ ...facts, dirty: true }),
      owns: () => true,
      render: () => {},
      settle: () => Promise.resolve(false),
      acquire: () => {},
      release: () => {},
    });
    await owner.submit("photo");
    expect(sent).toBe(false);
    expect(owner.view("photo").xmp.canSubmit).toBe(false);
  });
});
