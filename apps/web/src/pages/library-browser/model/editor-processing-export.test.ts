import { afterEach, describe, expect, test } from "bun:test";
import {
  createEditorProcessingExport,
  type ProcessingExportRecipe,
} from "./editor-processing-export.js";
import type { BrowserFetch } from "./access-session.js";
import type { ProcessingArtifactRecord } from "./processing-artifact.js";
import { parseProcessingExportWork } from "./processing-export.js";

type ExportRequest = {
  requestId: string;
  stepId?: string;
  expectedEditRevision?: string;
  expectedSourceRevision?: string;
};
function requestBody(body: RequestInit["body"]): string {
  if (typeof body !== "string") throw new Error("Expected a JSON request body");
  return body;
}
function requestUrl(input: RequestInfo | URL): string {
  return typeof input === "string"
    ? input
    : input instanceof URL
      ? input.href
      : input.url;
}

const work = (
  requestId: string,
  state = "failed",
  stepId = "captured-step",
) => ({
  photoId: "photo-1",
  requestId,
  stepId,
  state,
  module: "captured-module",
  recipeRevision: "captured-recipe",
  sourceRevision: "opaque\u0000source",
  artifactId: null,
  failureReason: state === "failed" ? "engine_error" : null,
  acceptedAt: 1,
  terminalAt: state === "accepted" ? null : 2,
  retainUntil: 3,
  parameters: { schemaVersion: "v1", tree: { quality: "captured" } },
  input: {
    kind: "original",
    photoId: "photo-1",
    sourceRevision: "opaque\u0000source",
  },
  bundleId: "bundle",
});
const artifact: ProcessingArtifactRecord = {
  artifactId: "a".repeat(64),
  photoId: "photo-1",
  stepId: "captured-step",
  module: "captured-module",
  adapterSchemaVersion: "v1",
  parameters: { schemaVersion: "v1", tree: {} },
  input: {
    binding: { kind: "original", photoId: "photo-1", sourceRevision: "source" },
    sha256: "b".repeat(64),
    byteLength: 1,
  },
  outputContract: {
    format: "png",
    precision: "uint8",
    colorSpace: "srgb",
    transfer: "srgb",
    geometry: { width: 1, height: 1 },
    encoding: "png",
  },
  bundleId: "bundle",
  sha256: "c".repeat(64),
  byteLength: 1,
  filename: "retained-result.png",
  publishedAt: "2026-10-01T00:00:00Z",
  expiresAt: "2099-10-01T00:00:00Z",
  orientation: "top-left",
  iccEmbedded: true,
  sampleFormat: "uint8",
};
const owners: Array<{ reset(): void }> = [];
afterEach(() => {
  for (const owner of owners.splice(0)) owner.reset();
});
function fixture(
  fetcher: (
    input: RequestInfo | URL,
    init?: RequestInit,
  ) => Response | Promise<Response>,
) {
  let current = "photo-1";
  let dirty = false;
  const artifacts: ProcessingArtifactRecord[] = [];
  const step = {
    stepId: "current-step",
    module: "current-module",
    input: {
      kind: "original" as const,
      photoId: "photo-1",
      sourceRevision: "source",
    },
    parameters: { schemaVersion: "v1", tree: {} },
  };
  const recipe: ProcessingExportRecipe = {
    read: {
      sourceRevision: "source\u0000revision",
      recipe: {
        photoId: "photo-1",
        revision: "recipe",
        sourceRevision: "source\u0000revision",
        currentStepId: step.stepId,
        steps: [step],
      },
    },
    artifacts,
    target: () => ({ kind: "step", step }),
    dirty: () => dirty,
    retainArtifact: (record) => {
      if (!artifacts.some((item) => item.artifactId === record.artifactId))
        artifacts.push(record);
    },
    fetchEditorArtifact: () => {
      artifacts.push(artifact);
      return Promise.resolve();
    },
  };
  const browserFetch: BrowserFetch = async (input, init) => {
    const response = await fetcher(input, init);
    return response;
  };
  const owner = createEditorProcessingExport(browserFetch, recipe, {
    editorOwnsPhoto: (id) => id === current,
    renderEditor: () => {},
    processingAvailable: () => true,
    describeEditRefusal: () => Promise.resolve("refused"),
  });
  owners.push(owner);
  return {
    owner,
    artifacts,
    navigate: (id: string) => {
      owner.reset();
      artifacts.length = 0;
      current = id;
    },
    dirty: () => {
      dirty = true;
    },
  };
}

describe("retained Processing Exports", () => {
  test("historical outputs remain readonly cards and expired bytes are not fetched", async () => {
    let requests = 0;
    const historical = {
      exportId: "legacy",
      target: "development-tiff",
      state: "succeeded",
      createdAt: "2000-01-01T00:00:00Z",
      artifact: {
        exportId: "legacy",
        target: "development-tiff",
        stage: "develop",
        contentType: "image/tiff",
        width: 10,
        height: 20,
        profileIdentity: "ProPhoto RGB",
        byteLength: 1,
        sha256: "a".repeat(64),
        expiresAt: "2000-01-02T00:00:00Z",
        filename: "historical.tiff",
        orientation: "top-left",
        sampleFormat: "float32",
        colorSpace: "prophoto-rgb",
        iccEmbedded: true,
      },
    };
    const f = fixture(() => {
      requests += 1;
      return Response.json({
        photoId: "photo-1",
        exports: [],
        artifacts: [],
        historicalExports: [historical],
      });
    });
    await f.owner.load("photo-1");
    const card = f.owner.view().historicalExports[0];
    expect(card?.filename).toBe("historical.tiff");
    expect(card?.type).toBe("image/tiff");
    expect(card?.canDownload).toBe(false);
    expect(f.owner.view().processingArtifacts).toEqual([]);
    expect(f.artifacts).toEqual([]);
    await f.owner.downloadHistorical("photo-1", "legacy");
    expect(requests).toBe(1);
  });

  test("capture parser preserves opaque source bytes and enforces published identity bounds", () => {
    const captured = {
      ...work("request"),
      sourceRevision: `\u0000${"x".repeat(16_383)}`,
    };
    expect(parseProcessingExportWork(captured)?.sourceRevision).toBe(
      captured.sourceRevision,
    );
    expect(
      parseProcessingExportWork({
        ...captured,
        sourceRevision: `${captured.sourceRevision}x`,
      }),
    ).toBeUndefined();
    expect(
      parseProcessingExportWork({
        ...captured,
        recipeRevision: "r".repeat(129),
      }),
    ).toBeUndefined();
    expect(
      parseProcessingExportWork({ ...captured, requestId: "r".repeat(129) }),
    ).toBeUndefined();
    expect(
      parseProcessingExportWork({
        ...captured,
        input: { kind: "artifact", artifactId: "id" },
      }),
    ).toBeUndefined();
  });

  test("restores every retained task and immutable artifact on navigation", async () => {
    const f = fixture(() =>
      Response.json({
        photoId: "photo-1",
        exports: [work("latest", "cancelled"), work("older")],
        artifacts: [artifact],
      }),
    );
    await f.owner.load("photo-1");
    expect(
      f.owner
        .view()
        .processingExports.map((item) => [item.requestId, item.canRetry]),
    ).toEqual([
      ["latest", true],
      ["older", true],
    ]);
    expect(f.owner.view().processingArtifacts[0]?.filename).toBe(
      "retained-result.png",
    );
    expect(f.artifacts[0]?.artifactId).toBe(artifact.artifactId);
    f.navigate("photo-2");
    expect(f.owner.view().processingArtifacts).toEqual([]);
    f.navigate("photo-1");
    await f.owner.load("photo-1");
    expect(f.owner.view().processingExports).toHaveLength(2);
  });

  test("acceptance returns before execution completes and enables cancellation", async () => {
    let submittedPath = "";
    let submittedBody: ExportRequest | undefined;
    const f = fixture((_path, options) => {
      const body = JSON.parse(requestBody(options?.body)) as ExportRequest;
      submittedPath = requestUrl(_path);
      submittedBody = body;
      return Response.json(
        {
          outcome: "accepted",
          receipt: work(body.requestId, "accepted", body.stepId),
        },
        { status: 202 },
      );
    });
    await f.owner.submit("photo-1");
    expect(f.owner.view().state).toBe("running");
    expect(f.owner.view().canCancel).toBe(true);
    expect(f.owner.view().processingExports[0]?.state).toBe("accepted");
    expect(submittedPath).toBe("/api/photos/photo-1/edit/export");
    expect(submittedBody).toMatchObject({ expectedEditRevision: "recipe" });
  });

  test("retry of retained failed work captures the old request even when the current draft changes", async () => {
    const calls: Array<{ path: string; body: string }> = [];
    const f = fixture((path, options) => {
      if (options?.method !== "POST")
        return Response.json({
          photoId: "photo-1",
          exports: [work("old")],
          artifacts: [],
        });
      calls.push({ path: requestUrl(path), body: requestBody(options.body) });
      const body = JSON.parse(requestBody(options.body)) as ExportRequest;
      return Response.json(
        { outcome: "accepted", receipt: work(body.requestId, "accepted") },
        { status: 202 },
      );
    });
    await f.owner.load("photo-1");
    f.dirty();
    await f.owner.retry("photo-1", "old");
    expect(calls[0]?.path).toBe(
      "/api/photos/photo-1/processing-exports/old/retry",
    );
    const body = JSON.parse(calls[0]!.body) as ExportRequest;
    expect(Object.keys(body)).toEqual(["requestId"]);
    expect(body.requestId).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/,
    );
    expect(f.owner.view().processingExports[0]?.stepId).toBe("captured-step");
  });

  test("an uncertain submit replays exact bytes and identity after leaving and returning", async () => {
    const calls: Array<{ path: string; body: string }> = [];
    const f = fixture((path, options) => {
      if (options?.method !== "POST")
        return Response.json({
          photoId: "photo-1",
          exports: [],
          artifacts: [],
        });
      calls.push({ path: requestUrl(path), body: requestBody(options.body) });
      throw new Error("response lost");
    });
    await f.owner.submit("photo-1");
    expect(f.owner.view().state).toBe("outcome-unknown");
    f.navigate("photo-2");
    f.navigate("photo-1");
    await f.owner.load("photo-1");
    f.dirty();
    await f.owner.retry("photo-1");
    expect(calls).toHaveLength(2);
    expect(calls[1]).toEqual(calls[0]);
    expect(
      (JSON.parse(calls[1]!.body) as ExportRequest).expectedEditRevision,
    ).toBe("recipe");
    expect(f.owner.view().canSubmit).toBe(false);
  });

  test("uncertain captured retry replays its new UUID instead of admitting another retry", async () => {
    const calls: string[] = [];
    const f = fixture((_path, options) => {
      if (options?.method !== "POST")
        return Response.json({
          photoId: "photo-1",
          exports: [work("cancelled", "cancelled")],
          artifacts: [],
        });
      calls.push(requestBody(options.body));
      throw new Error("response lost");
    });
    await f.owner.load("photo-1");
    await f.owner.retry("photo-1", "cancelled");
    f.navigate("photo-2");
    f.navigate("photo-1");
    await f.owner.load("photo-1");
    await f.owner.retry("photo-1");
    expect(calls).toHaveLength(2);
    expect(calls[1]).toBe(calls[0]);
  });

  for (const code of [
    "module_parameters_unavailable",
    "resource_unavailable",
    "processing_unavailable",
  ]) {
    test(`captured retry settles explicit ${code} without a replay marker`, async () => {
      const calls: string[] = [];
      const f = fixture((_path, options) => {
        if (options?.method !== "POST")
          return Response.json({
            photoId: "photo-1",
            exports: [work("cancelled", "cancelled")],
            artifacts: [],
          });
        calls.push(requestBody(options.body));
        return Response.json(
          {
            error: {
              code,
              message: "Captured processing bundle is unavailable",
              details: { reasonCode: "captured_bundle_unavailable" },
            },
          },
          { status: 503 },
        );
      });
      await f.owner.load("photo-1");
      await f.owner.retry("photo-1", "cancelled");
      expect(f.owner.view().state).toBe("failed");
      expect(f.owner.unresolved("photo-1")).toBe(false);
      expect(f.owner.view().canSubmit).toBe(true);
      await f.owner.retry("photo-1", "cancelled");
      expect(calls).toHaveLength(2);
      expect(calls[1]).not.toBe(calls[0]);
    });
  }

  for (const code of ["outcome_unknown", "storage", "unrecognized_failure"]) {
    test(`captured retry preserves exact replay for uncertain 503 ${code}`, async () => {
      const calls: { path: string; body: string }[] = [];
      const f = fixture((path, options) => {
        if (options?.method !== "POST")
          return Response.json({
            photoId: "photo-1",
            exports: [work("cancelled", "cancelled")],
            artifacts: [],
          });
        calls.push({ path: requestUrl(path), body: requestBody(options.body) });
        return Response.json({ error: { code } }, { status: 503 });
      });
      await f.owner.load("photo-1");
      await f.owner.retry("photo-1", "cancelled");
      expect(f.owner.view().state).toBe("outcome-unknown");
      expect(f.owner.unresolved("photo-1")).toBe(true);
      expect(f.owner.view().canSubmit).toBe(false);
      await f.owner.check("photo-1");
      expect(calls).toHaveLength(2);
      expect(calls[1]).toEqual(calls[0]);
    });
  }

  test("cancellation reconciles the authoritative terminal decision", async () => {
    const paths: string[] = [];
    const f = fixture((path, options) => {
      paths.push(requestUrl(path));
      if (options?.method === "POST")
        return Response.json({ outcome: "cancelled" });
      if (requestUrl(path).endsWith("/running"))
        return Response.json(work("running", "cancelled"));
      return Response.json({
        photoId: "photo-1",
        exports: [work("running", "executing")],
        artifacts: [],
      });
    });
    await f.owner.load("photo-1");
    await f.owner.cancel("photo-1", "running");
    expect(paths).toContain(
      "/api/photos/photo-1/processing-exports/running/cancel",
    );
    expect(f.owner.view().state).toBe("cancelled");
    expect(f.owner.view().canRetry).toBe(true);
  });
  test("successful terminal exact replay adopts the retained artifact", async () => {
    let first = true;
    const f = fixture(() => {
      if (first) {
        first = false;
        throw new Error("response lost");
      }
      return Response.json(
        { artifact: { ...artifact, stepId: "current-step" }, replayed: true },
        { status: 201 },
      );
    });
    await f.owner.submit("photo-1");
    await f.owner.check("photo-1");
    expect(f.owner.unresolved("photo-1")).toBe(false);
    expect(f.owner.view().state).toBe("succeeded");
    expect(f.owner.view().processingArtifact?.filename).toBe(
      "retained-result.png",
    );
  });

  test("failed terminal exact replay restores retryable captured work", async () => {
    let first = true;
    const f = fixture((_path, options) => {
      const body = JSON.parse(requestBody(options?.body)) as ExportRequest;
      if (first) {
        first = false;
        throw new Error("response lost");
      }
      return Response.json(
        {
          error: {
            code: "export_terminal",
            details: {
              receipt: work(body.requestId, "failed", body.stepId),
              replayed: true,
            },
          },
        },
        { status: 409 },
      );
    });
    await f.owner.submit("photo-1");
    await f.owner.check("photo-1");
    expect(f.owner.unresolved("photo-1")).toBe(false);
    expect(f.owner.view().state).toBe("failed");
    expect(f.owner.view().processingExports[0]?.canRetry).toBe(true);
  });

  test("a retained receipt cannot resolve uncertain admission without exact-body replay", async () => {
    let captured: { requestId: string; stepId: string } | undefined;
    let submits = 0;
    const f = fixture((_path, options) => {
      if (options?.method === "POST") {
        submits += 1;
        captured = JSON.parse(requestBody(options.body)) as {
          requestId: string;
          stepId: string;
        };
        if (submits === 1) throw new Error("response lost");
        return Response.json(
          {
            outcome: "replayed",
            receipt: work(captured.requestId, "executing", captured.stepId),
          },
          { status: 202 },
        );
      }
      return Response.json({
        photoId: "photo-1",
        exports: [work(captured!.requestId, "executing", captured!.stepId)],
        artifacts: [],
      });
    });
    await f.owner.submit("photo-1");
    expect(f.owner.unresolved("photo-1")).toBe(true);
    f.navigate("photo-2");
    f.navigate("photo-1");
    await f.owner.load("photo-1");
    expect(f.owner.unresolved("photo-1")).toBe(true);
    expect(f.owner.view().state).toBe("outcome-unknown");
    await f.owner.check("photo-1");
    expect(f.owner.unresolved("photo-1")).toBe(false);
    expect(f.owner.view().state).toBe("running");
    expect(submits).toBe(2);
  });

  test("explicit artifact inspection appears in cards and expiry prevents downloads", async () => {
    let downloads = 0;
    const f = fixture(() => {
      downloads += 1;
      return Response.json({ photoId: "photo-1", exports: [], artifacts: [] });
    });
    await f.owner.load("photo-1");
    downloads = 0;
    f.artifacts.push({ ...artifact, expiresAt: "2000-01-01T00:00:00Z" });
    const card = f.owner.view().processingArtifacts[0];
    expect(card?.filename).toBe("retained-result.png");
    expect(card?.isExpired).toBe(true);
    expect(card?.isStale).toBe(true);
    expect(card?.canDownload).toBe(false);
    await f.owner.downloadArtifact("photo-1", artifact.artifactId);
    expect(downloads).toBe(0);
  });

  test("download uses published filename after metadata and SHA verification", async () => {
    const bytes = new Uint8Array([7]);
    const digest = await crypto.subtle.digest("SHA-256", bytes);
    const sha256 = Array.from(new Uint8Array(digest), (byte) =>
      byte.toString(16).padStart(2, "0"),
    ).join("");
    const published = { ...artifact, sha256 };
    const headers = new Headers({
      "slipstream-artifact-id": published.artifactId,
      "slipstream-artifact-photo-id": published.photoId,
      "slipstream-artifact-step-id": published.stepId,
      "slipstream-artifact-module": published.module,
      "slipstream-artifact-adapter-schema-version":
        published.adapterSchemaVersion,
      "slipstream-artifact-bundle-id": published.bundleId,
      "slipstream-artifact-width": "1",
      "slipstream-artifact-height": "1",
      "slipstream-artifact-byte-length": "1",
      "slipstream-artifact-sha256": sha256,
      "slipstream-artifact-filename": published.filename,
      "slipstream-artifact-published-at": published.publishedAt,
      "slipstream-artifact-expires-at": published.expiresAt,
      "slipstream-artifact-orientation": published.orientation,
      "slipstream-artifact-icc-embedded": String(published.iccEmbedded),
      "slipstream-artifact-sample-format": published.sampleFormat,
    });
    let downloaded = "";
    const originalDocument = Object.getOwnPropertyDescriptor(
      globalThis,
      "document",
    );
    Object.defineProperty(globalThis, "document", {
      configurable: true,
      value: {
        createElement: () => ({
          href: "",
          download: "",
          click() {
            downloaded = this.download;
          },
        }),
      },
    });
    try {
      let corrupt = false;
      const f = fixture((path) =>
        requestUrl(path).endsWith("/bytes")
          ? new Response(new Uint8Array([corrupt ? 8 : 7]), { headers })
          : Response.json({
              photoId: "photo-1",
              exports: [],
              artifacts: [published],
            }),
      );
      await f.owner.load("photo-1");
      await f.owner.downloadArtifact("photo-1", published.artifactId);
      expect(downloaded).toBe("retained-result.png");
      downloaded = "";
      corrupt = true;
      await f.owner.downloadArtifact("photo-1", published.artifactId);
      expect(downloaded).toBe("");
      expect(f.owner.view().note).toContain("digest");
    } finally {
      if (originalDocument)
        Object.defineProperty(globalThis, "document", originalDocument);
      else Reflect.deleteProperty(globalThis, "document");
    }
  });

  test("a late retained read cannot replace another Photo's cards", async () => {
    const { promise, resolve } = Promise.withResolvers<Response>();
    const f = fixture(() => promise);
    const loading = f.owner.load("photo-1");
    f.navigate("photo-2");
    resolve(
      Response.json({
        photoId: "photo-1",
        exports: [work("old")],
        artifacts: [artifact],
      }),
    );
    await loading;
    expect(f.owner.view().processingExports).toEqual([]);
    expect(f.artifacts).toEqual([]);
  });
});
