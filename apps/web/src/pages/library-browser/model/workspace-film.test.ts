import { describe, expect, test } from "bun:test";
import type { ProcessingModuleDescription } from "../api/processing-modules.js";
import {
  parseWorkspaceFilmRead,
  type WorkspaceFilmFile,
} from "../api/workspace-film.js";
import {
  canContinueFilm,
  filmStepForFile,
  filmSaveBody,
  createWorkspaceFilmOwner,
} from "./workspace-film.js";

const module: ProcessingModuleDescription = {
  id: { name: "spektrafilm", adapterVersion: "1" },
  parameterVersions: ["film-1"],
  parameterSchema: {
    type: "object",
    default: {
      camera: {},
      enlarger: {},
      scanner: {},
      film: {},
      print: {},
      input: {},
      output: { format: "jpeg" },
    },
  },
  automaticAdjustments: [],
  admittedInputs: [
    {
      format: "image/tiff",
      colorSpace: "prophoto-rgb",
      transferFunction: "linear",
      precisionBits: 32,
      width: 0,
      height: 0,
    },
  ],
  admittedOutputs: [],
  limits: { maxOutputPixels: 1000, maxInputBytes: 1000000 },
  availability: { state: "ready", refusalReasons: [] },
};
const file: WorkspaceFilmFile = {
  workspaceId: "workspace-1",
  artifactId: "completed-1",
  revision: "bytes-1",
  relativePath: "outputs/portrait.tif",
  imageContract: {
    format: "image/tiff",
    precision: "float32",
    colorSpace: "prophoto-rgb",
    transfer: "linear",
    geometry: { width: 6336, height: 9504 },
    encoding: "deflate",
  },
};
const empty = { photoId: "photo-1", sourceRevision: "", recipe: null };

function fixture(failSave = false) {
  const calls: { path: string; body?: string }[] = [];
  let saved: Record<string, unknown> = empty;
  const owner = createWorkspaceFilmOwner(
    (input, init) => {
      const path =
          typeof input === "string"
            ? input
            : input instanceof URL
              ? input.href
              : input.url,
        body = typeof init?.body === "string" ? init.body : undefined;
      calls.push({ path, ...(body === undefined ? {} : { body }) });
      if (init?.method !== "POST") return Promise.resolve(Response.json(saved));
      const request = JSON.parse(body ?? "{}") as Record<string, unknown>;
      if (path.endsWith("processing-recipe")) {
        if (failSave) {
          failSave = false;
          return Promise.reject(new Error("connection lost"));
        }
        saved = {
          photoId: "photo-1",
          sourceRevision: "",
          recipe: {
            photoId: "photo-1",
            revision: "saved-1",
            sourceRevision: "",
            currentStepId: request.currentStepId,
            steps: request.steps,
          },
        };
        return Promise.resolve(Response.json(saved));
      }
      return Promise.resolve(
        Response.json(
          {
            photoId: "photo-1",
            requestId: request.requestId,
            receipt: {
              photoId: "photo-1",
              requestId: request.requestId,
              stepId: request.stepId,
              state: "accepted",
            },
          },
          { status: 202 },
        ),
      );
    },
    "photo-1",
    () => {},
  );
  return { calls, owner };
}

describe("workspace Film handoff", () => {
  test("requires verified scene-linear floating-point image facts and accepts portrait geometry", () => {
    expect(canContinueFilm(file.imageContract, module)).toBe(true);
    for (const change of [
      { precision: "uint16" },
      { colorSpace: "srgb" },
      { transfer: "srgb" },
      { format: "image/jpeg" },
      { geometry: { width: 0, height: 1 } },
    ])
      expect(
        canContinueFilm({ ...file.imageContract, ...change }, module),
      ).toBe(false);
    expect(
      canContinueFilm(file.imageContract, {
        ...module,
        availability: {
          state: "unavailable",
          refusalReasons: ["not installed"],
        },
      }),
    ).toBe(false);
  });
  test("copies exact revision and complete discovered defaults without mutating discovery", () => {
    const read = parseWorkspaceFilmRead(empty, "photo-1");
    const step = filmStepForFile(read, file, module);
    expect(step.input).toEqual({
      kind: "workspace-artifact",
      workspaceId: file.workspaceId,
      artifactId: file.artifactId,
      revision: file.revision,
      contract: file.imageContract,
    });
    expect(step.parameters.tree).toEqual(
      (module.parameterSchema as Record<string, unknown>).default,
    );
    expect(step.parameters.tree).not.toBe(
      (module.parameterSchema as Record<string, unknown>).default,
    );
  });
  test("preserves unrelated retained state and empty observed source guard", () => {
    const darktable = {
      stepId: "develop-1",
      module: "darktable",
      input: { kind: "original", photoId: "photo-1", sourceRevision: "old" },
      parameters: { schemaVersion: "unknown", tree: { private: [1, 2, 3] } },
    };
    const read = parseWorkspaceFilmRead(
      {
        photoId: "photo-1",
        sourceRevision: "",
        recipe: {
          photoId: "photo-1",
          revision: "old-1",
          sourceRevision: "old",
          currentStepId: "develop-1",
          steps: [darktable],
        },
      },
      "photo-1",
    );
    const body = JSON.parse(
      filmSaveBody(read, filmStepForFile(read, file, module), "save-1"),
    ) as {
      steps: unknown[];
      expectedRecipeRevision: string;
      expectedSourceRevision: string;
    };
    expect(body.steps[0]).toEqual(darktable);
    expect(body.expectedRecipeRevision).toBe("old-1");
    expect(body.expectedSourceRevision).toBe("");
    expect(Object.keys(body).sort()).toEqual(
      [
        "requestId",
        "expectedRecipeRevision",
        "expectedSourceRevision",
        "currentStepId",
        "steps",
      ].sort(),
    );
  });
  test("save then export captures confirmed exact settings and performs no upload or upstream render", async () => {
    const { owner, calls } = fixture();
    await owner.open(file, module);
    owner.setParameters({
      ...((module.parameterSchema as Record<string, unknown>)
        .default as object),
      output: { format: "jpeg", quality: 90 },
    });
    await owner.export();
    const writes = calls.filter((call) => call.body);
    expect(writes.length).toBe(2);
    expect(writes[0]!.path).toEndWith("processing-recipe");
    expect(writes[1]!.path).toEndWith("processing-exports");
    const body = JSON.parse(writes[1]!.body!) as Record<string, unknown>;
    expect(body.stepId).toBe("film-1");
    expect(body.expectedRecipeRevision).toBe("saved-1");
    expect(body.expectedSourceRevision).toBe("");
    expect(
      calls.some((call) => /upload|darktable|preview/.test(call.path)),
    ).toBe(false);
    owner.dispose();
  });
  test("unknown save outcome retains and replays identical bytes before export", async () => {
    const { owner, calls } = fixture(true);
    await owner.open(file, module);
    await owner.export();
    expect(owner.view().uncertain).toBe(true);
    expect(calls.filter((call) => call.body).length).toBe(1);
    await owner.reconcile();
    const writes = calls.filter((call) => call.body);
    expect(writes[1]!.body).toBe(writes[0]!.body);
    expect(owner.view().dirty).toBe(false);
    owner.dispose();
  });
  test("several saved Film edits require explicit choice", () => {
    const first = filmStepForFile(
      parseWorkspaceFilmRead(empty, "photo-1"),
      file,
      module,
    );
    const second = {
      ...first,
      stepId: "film-2",
      parameters: { ...first.parameters, tree: { scanner: { exposure: 2 } } },
    };
    const read = parseWorkspaceFilmRead(
      {
        ...empty,
        recipe: {
          photoId: "photo-1",
          revision: "r1",
          sourceRevision: "",
          currentStepId: first.stepId,
          steps: [first, second],
        },
      },
      "photo-1",
    );
    expect(() => filmStepForFile(read, file, module)).toThrow(
      "Several saved Film edits",
    );
    expect(filmStepForFile(read, file, module, "film-2")).toEqual(second);
  });
  test("conflict retains draft and blocks render until inspect and explicit reapply", async () => {
    let fail = true;
    const writes: string[] = [];
    const owner = createWorkspaceFilmOwner(
      (_input, init) => {
        if (init?.method !== "POST")
          return Promise.resolve(Response.json(empty));
        if (typeof init.body !== "string")
          throw new Error("Expected string request body");
        writes.push(init.body);
        if (fail) {
          fail = false;
          return Promise.resolve(
            Response.json(
              {
                error: {
                  code: "conflict",
                  message: "Canonical state changed.",
                },
              },
              { status: 409 },
            ),
          );
        }
        const request = JSON.parse(init.body) as Record<string, unknown>;
        return Promise.resolve(
          Response.json({
            ...empty,
            recipe: {
              photoId: "photo-1",
              revision: "r2",
              sourceRevision: "",
              currentStepId: request.currentStepId,
              steps: request.steps,
            },
          }),
        );
      },
      "photo-1",
      () => {},
    );
    await owner.open(file, module);
    await owner.save();
    expect(owner.view().conflict).toBe(true);
    expect(owner.view().dirty).toBe(true);
    await owner.export();
    await owner.reapply();
    expect(writes.length).toBe(1);
    await owner.inspectConflict();
    await owner.reapply();
    expect(writes.length).toBe(2);
    expect(owner.view().dirty).toBe(false);
    owner.dispose();
  });
  test("late state response cannot publish after dispose", async () => {
    let resolve!: (response: Response) => void;
    let changed = 0;
    const owner = createWorkspaceFilmOwner(
      () =>
        new Promise<Response>((done) => {
          resolve = done;
        }),
      "photo-1",
      () => {
        changed += 1;
      },
    );
    const pending = owner.open(file, module);
    owner.dispose();
    const count = changed;
    resolve(Response.json(empty));
    await pending;
    expect(changed).toBe(count);
    expect(owner.view().step).toBeUndefined();
  });
});
