import { expect, test } from "bun:test";
import { createEditorEditState } from "./editor-edit-state.js";

function requestUrl(input: RequestInfo | URL): string {
  return typeof input === "string"
    ? input
    : input instanceof URL
      ? input.href
      : input.url;
}

function requestBody(init: RequestInit | undefined): Record<string, unknown> {
  if (typeof init?.body !== "string")
    throw new Error("Expected a JSON request body");
  const value: unknown = JSON.parse(init.body);
  if (typeof value !== "object" || value === null || Array.isArray(value))
    throw new Error("Expected a JSON object request body");
  return value as Record<string, unknown>;
}

function state(revision = "r1", ev = 0) {
  return {
    photoId: "photo/1",
    editRevision: revision,
    sourceRevision: "source",
    currentSourceRevision: "source",
    sourceAvailable: true,
    requiresRebind: false,
    canSave: true,
    canPreview: true,
    canExport: true,
    current: {
      engine: "darktable",
      input: { kind: "original", photoId: "photo/1", sourceRevision: "source" },
      controls: { exposure: { ev } },
    },
  };
}

for (const outcome of ["saved", "replayed", "unchanged"]) {
  test(`primary exposure ${outcome} re-reads the edit and blocks processing until confirmation`, async () => {
    const requests: { path: string; body: Record<string, unknown> }[] = [];
    const confirmation = Promise.withResolvers<Response>();
    let reads = 0;
    let compatibilityRead = false;
    let previewRequested = false;
    const editor = createEditorEditState(
      async (input, options) => {
        const path = requestUrl(input);
        if (options?.method === "POST") {
          requests.push({ path, body: requestBody(options) });
          return Response.json({ outcome });
        }
        reads += 1;
        return reads === 1 ? Response.json(state()) : confirmation.promise;
      },
      {
        owns: () => true,
        render: () => {},
        markStale: () => {},
        refreshCompatibility: () => {
          compatibilityRead = true;
          return Promise.resolve();
        },
        requestPreview: () => {
          previewRequested = true;
          return Promise.resolve();
        },
        describeRefusal: () => Promise.resolve("refused"),
      },
    );
    await editor.load("photo/1");
    const saved = editor.setExposure("photo/1", 1.5);
    await Promise.resolve();
    await Promise.resolve();
    expect(editor.blocked).toBe(true);
    expect(previewRequested).toBe(false);
    expect(requests).toHaveLength(1);
    expect(requests[0]?.path).toBe("/api/photos/photo%2F1/edit/set");
    expect(requests[0]?.body).toMatchObject({
      expectedEditRevision: "r1",
      target: "darktable.exposure",
      control: "ev",
      value: 1.5,
    });
    expect(requests[0]?.body["requestId"]).toMatch(/^[0-9a-f-]{36}$/);
    confirmation.resolve(Response.json(state("r2", 1.5)));
    await saved;
    expect(editor.read?.editRevision).toBe("r2");
    expect(editor.read?.current?.exposureEv).toBe(1.5);
    expect(editor.blocked).toBe(false);
    expect(compatibilityRead).toBe(true);
    expect(previewRequested).toBe(true);
    expect(
      requests.every((request) => !request.path.endsWith("processing-recipe")),
    ).toBe(true);
  });
}

test("ordinary reset writes the qualified control through edit/reset", async () => {
  let request: { path: string; body: Record<string, unknown> } | undefined;
  const editor = createEditorEditState(
    (input, options) => {
      if (options?.method === "POST") {
        request = { path: requestUrl(input), body: requestBody(options) };
        return Promise.resolve(Response.json({ outcome: "saved" }));
      }
      return Promise.resolve(Response.json(state()));
    },
    {
      owns: () => true,
      render: () => {},
      markStale: () => {},
      refreshCompatibility: () => Promise.resolve(),
      requestPreview: () => Promise.resolve(),
      describeRefusal: () => Promise.resolve("refused"),
    },
  );
  await editor.load("photo/1");
  await editor.resetExposure("photo/1");
  expect(request?.path).toBe("/api/photos/photo%2F1/edit/reset");
  expect(request?.body).toMatchObject({
    expectedEditRevision: "r1",
    target: "darktable.exposure",
    control: "ev",
  });
  expect(typeof request?.body["requestId"]).toBe("string");
});

test("an unconfirmed write keeps processing blocked until a successful read", async () => {
  const editor = createEditorEditState(
    (_input, options) => {
      if (options?.method === "POST")
        return Promise.reject(new Error("response lost"));
      return Promise.resolve(Response.json(state()));
    },
    {
      owns: () => true,
      render: () => {},
      markStale: () => {},
      refreshCompatibility: () => Promise.resolve(),
      requestPreview: () => Promise.resolve(),
      describeRefusal: () => Promise.resolve("refused"),
    },
  );
  await editor.load("photo/1");
  await editor.setExposure("photo/1", 2);
  expect(editor.blocked).toBe(true);
  await editor.load("photo/1");
  expect(editor.blocked).toBe(false);
});

test("primary exposure preserves the current retained artifact input", async () => {
  let body: Record<string, unknown> | undefined;
  const editor = createEditorEditState(
    (_input, options) => {
      if (options?.method === "POST") {
        body = requestBody(options);
        return Promise.resolve(Response.json({ outcome: "saved" }));
      }
      const read = state();
      return Promise.resolve(
        Response.json({
          ...read,
          current: {
            ...read.current,
            input: {
              kind: "artifact",
              artifactId: "retained-1",
              contract: { format: "tiff" },
            },
          },
        }),
      );
    },
    {
      owns: () => true,
      render: () => {},
      markStale: () => {},
      refreshCompatibility: () => Promise.resolve(),
      requestPreview: () => Promise.resolve(),
      describeRefusal: () => Promise.resolve("refused"),
    },
  );
  await editor.load("photo/1");
  await editor.setExposure("photo/1", 0.5);
  expect(body).toMatchObject({ value: 0.5 });
  expect(body).not.toHaveProperty("from");
});

test("first ordinary exposure change explicitly binds the Original", async () => {
  let body: Record<string, unknown> | undefined;
  const editor = createEditorEditState(
    (_input, options) => {
      if (options?.method === "POST") {
        body = requestBody(options);
        return Promise.resolve(Response.json({ outcome: "saved" }));
      }
      return Promise.resolve(
        Response.json({
          ...state(),
          editRevision: null,
          current: null,
          canSave: false,
          canPreview: false,
          canExport: false,
        }),
      );
    },
    {
      owns: () => true,
      render: () => {},
      markStale: () => {},
      refreshCompatibility: () => Promise.resolve(),
      requestPreview: () => Promise.resolve(),
      describeRefusal: () => Promise.resolve("refused"),
    },
  );
  await editor.load("photo/1");
  await editor.setExposure("photo/1", 0.5);
  expect(body).toMatchObject({
    expectedEditRevision: null,
    from: "original",
    value: 0.5,
  });
});

test("concurrent refreshes share a read and cannot publish pre-write state", async () => {
  let reads = 0;
  const firstRead = Promise.withResolvers<Response>();
  const mutation = Promise.withResolvers<Response>();
  const editor = createEditorEditState(
    async (_input, options) => {
      if (options?.method === "POST") return mutation.promise;
      reads += 1;
      return reads === 1 ? firstRead.promise : Response.json(state("r2", 2));
    },
    {
      owns: () => true,
      render: () => {},
      markStale: () => {},
      refreshCompatibility: () => Promise.resolve(),
      requestPreview: () => Promise.resolve(),
      describeRefusal: () => Promise.resolve("refused"),
    },
  );
  const initialRead = editor.load("photo/1");
  const concurrentRead = editor.load("photo/1");
  expect(concurrentRead).toBe(initialRead);
  expect(reads).toBe(1);
  firstRead.resolve(Response.json(state()));
  await initialRead;
  const change = editor.setExposure("photo/1", 2);
  await editor.load("photo/1");
  expect(reads).toBe(1);
  expect(editor.blocked).toBe(true);
  mutation.resolve(Response.json({ outcome: "saved" }));
  await change;
  expect(reads).toBe(2);
  expect(editor.read?.current?.exposureEv).toBe(2);
  expect(editor.blocked).toBe(false);
});
