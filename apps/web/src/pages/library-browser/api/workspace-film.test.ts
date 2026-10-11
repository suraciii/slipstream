import { describe, expect, test } from "bun:test";
import {
  parseWorkspaceFilmRead,
  workspaceFilmJson,
  WorkspaceFilmError,
} from "./workspace-film.js";

describe("Film retained adapter wire", () => {
  test("reads workspace bindings and unknown retained settings verbatim", () => {
    const input = {
      kind: "workspace-artifact",
      workspaceId: "w1",
      artifactId: "a1",
      revision: "r1",
      contract: { format: "image/tiff", precision: "float32" },
    };
    const step = {
      stepId: "film-1",
      module: "spektrafilm",
      input,
      parameters: {
        schemaVersion: "old-version",
        tree: { unsupported: [1, 2, 3] },
      },
    };
    const parsed = parseWorkspaceFilmRead(
      {
        photoId: "p1",
        sourceRevision: "",
        recipe: {
          photoId: "p1",
          revision: "r2",
          sourceRevision: "",
          currentStepId: "film-1",
          steps: [step],
        },
      },
      "p1",
    );
    expect(parsed.recipe?.steps[0]).toEqual(step);
    expect(parsed.recipe?.steps[0]?.input).not.toBe(input);
  });
  test("keeps absent and saved empty recipes distinct", () => {
    expect(
      parseWorkspaceFilmRead(
        { photoId: "p1", sourceRevision: "", recipe: null },
        "p1",
      ).recipe,
    ).toBeNull();
    expect(
      parseWorkspaceFilmRead(
        {
          photoId: "p1",
          sourceRevision: "",
          recipe: {
            photoId: "p1",
            revision: "r1",
            sourceRevision: "",
            currentStepId: null,
            steps: [],
          },
        },
        "p1",
      ).recipe?.revision,
    ).toBe("r1");
    expect(() =>
      parseWorkspaceFilmRead(
        { photoId: "p2", sourceRevision: "", recipe: null },
        "p1",
      ),
    ).toThrow();
  });
  test("confirmed conflict is distinct from possibly admitted mutation", async () => {
    for (const [status, code, uncertain] of [
      [409, "conflict", false],
      [503, "outcome_unknown", true],
    ] as const) {
      try {
        await workspaceFilmJson(
          () =>
            Promise.resolve(
              Response.json(
                { error: { code, message: "Retain draft." } },
                { status },
              ),
            ),
          "/api/photos/p1/processing-recipe",
          { method: "POST", body: "{}" },
        );
        throw new Error("Expected service refusal");
      } catch (error) {
        expect(error).toBeInstanceOf(WorkspaceFilmError);
        expect((error as WorkspaceFilmError).code).toBe(code);
        expect((error as WorkspaceFilmError).uncertain).toBe(uncertain);
      }
    }
  });
});
