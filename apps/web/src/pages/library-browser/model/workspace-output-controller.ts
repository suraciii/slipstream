import type { BrowserFetch } from "./access-session.js";
import { isRecord } from "../api/guards.js";
import { randomUuid, blobSha256Hex } from "./browser-crypto.js";

export type OutputState =
  | "idle"
  | "submitting"
  | "outcome-unknown"
  | "succeeded"
  | "failed";
export type XmpArtifact = Readonly<{
  exportId: string;
  photoId: string;
  recipeRevision: string;
  sourceRevision: string;
  createdAt: string;
  expiresAt: string;
  filename: string;
  contentType: string;
  byteLength: number;
  sha256: string;
  supportNote: string;
}>;
export type XmpOutputView = Readonly<{
  state: OutputState;
  note: string;
  artifact: XmpArtifact | null;
  isStale: boolean;
  canSubmit: boolean;
  canDownload: boolean;
}>;
export type WorkspaceOutputsView = Readonly<{ xmp: XmpOutputView }>;
export type OutputFacts = Readonly<{
  recipeRevision: string | null;
  sourceRevision: string | null;
  stepId: string | null;
  saving: boolean;
  dirty: boolean;
  conflict: boolean;
}>;
type Submission = Readonly<{
  requestId: string;
  expectedRecipeVersion: string;
  expectedSourceRevision: string;
}>;
type Session = {
  artifact: XmpArtifact | null;
  state: OutputState;
  note: string;
  pending?: Submission;
  submitting: boolean;
  generation: number;
};
const expired = (value: string): boolean =>
  !Number.isFinite(Date.parse(value)) || Date.parse(value) <= Date.now();
export function parseXmpArtifact(
  value: unknown,
  photoId: string,
): XmpArtifact | undefined {
  if (
    !isRecord(value) ||
    value["photoId"] !== photoId ||
    value["target"] !== "edit-state-xmp" ||
    value["state"] !== "succeeded" ||
    !isRecord(value["artifact"])
  )
    return;
  const artifact = value["artifact"];
  for (const key of [
    "exportId",
    "recipeVersion",
    "sourceRevision",
    "createdAt",
    "expiresAt",
  ])
    if (typeof value[key] !== "string" || !value[key]) return;
  for (const key of ["filename", "contentType", "sha256"])
    if (typeof artifact[key] !== "string" || !artifact[key]) return;
  if (
    artifact["contentType"] !== "application/rdf+xml" ||
    !/^[a-f0-9]{64}$/.test(String(artifact["sha256"])) ||
    typeof artifact["byteLength"] !== "number" ||
    !Number.isSafeInteger(artifact["byteLength"]) ||
    artifact["byteLength"] <= 0 ||
    artifact["byteLength"] > 65536
  )
    return;
  const support = isRecord(value["parameterSupport"])
    ? value["parameterSupport"]
    : undefined;
  const names = (key: string): string =>
    support && Array.isArray(support[key])
      ? support[key]
          .filter((item): item is string => typeof item === "string")
          .join(", ")
      : "";
  return {
    photoId,
    exportId: String(value["exportId"]),
    recipeRevision: String(value["recipeVersion"]),
    sourceRevision: String(value["sourceRevision"]),
    createdAt: String(value["createdAt"]),
    expiresAt: String(value["expiresAt"]),
    filename: String(artifact["filename"]),
    contentType: String(artifact["contentType"]),
    sha256: String(artifact["sha256"]),
    byteLength: artifact["byteLength"],
    supportNote: `Standard XMP: ${names("standard") || "none"}. Slipstream metadata: ${names("slipstream") || "none"}. Unsupported: ${names("unsupported") || "not disclosed"}.`,
  };
}
const failureNote = async (response: Response): Promise<string> => {
  const value: unknown = await response.json().catch(() => undefined);
  return isRecord(value) &&
    isRecord(value["error"]) &&
    typeof value["error"]["message"] === "string"
    ? value["error"]["message"]
    : "The XMP action was refused. Reload to check the saved edit.";
};
export function createWorkspaceOutputController(
  fetcher: BrowserFetch,
  dependencies: Readonly<{
    facts(id: string): OutputFacts | undefined;
    owns(id: string): boolean;
    render(): void;
  }>,
) {
  const sessions = new Map<string, Session>();
  let photo: string | undefined;
  let scope = 0;
  const session = (id: string): Session => {
    let value = sessions.get(id);
    if (!value) {
      value = {
        artifact: null,
        state: "idle",
        note: "Not exported yet.",
        submitting: false,
        generation: 0,
      };
      sessions.set(id, value);
    }
    return value;
  };
  const owns = (id: string, stamp: number): boolean =>
    photo === id && scope === stamp && dependencies.owns(id);
  const render = (id: string): void => {
    if (photo === id && dependencies.owns(id)) dependencies.render();
  };
  const refresh = async (id: string): Promise<void> => {
    const value = session(id);
    const stamp = scope;
    const generation = ++value.generation;
    try {
      const response = await fetcher(
        `/api/photos/${encodeURIComponent(id)}/edit-state-exports`,
        { priority: "low" },
      );
      const body: unknown = await response.json().catch(() => undefined);
      if (!owns(id, stamp) || generation !== value.generation || value.pending)
        return;
      if (!response.ok) {
        value.note =
          "Retained XMP files could not be read. Reload to try again.";
        render(id);
        return;
      }
      if (!isRecord(body) || !Array.isArray(body["exports"])) return;
      const entries = body["exports"]
        .map((item) => parseXmpArtifact(item, id))
        .filter((item): item is XmpArtifact => item !== undefined)
        .sort((a, b) => b.createdAt.localeCompare(a.createdAt));
      value.artifact = entries[0] ?? null;
      value.state = value.artifact ? "succeeded" : "idle";
      value.note = value.artifact
        ? "Ready to download. XMP contains the saved Edit State, not rendered pixels."
        : "Not exported yet.";
      render(id);
    } catch {
      if (owns(id, stamp)) {
        value.note =
          "Retained XMP files could not be read. Reload to try again.";
        render(id);
      }
    }
  };
  const submit = async (id: string): Promise<void> => {
    const value = session(id);
    if (!dependencies.owns(id) || value.submitting) return;
    if (!value.pending) {
      const facts = dependencies.facts(id);
      if (
        !facts?.recipeRevision ||
        !facts.sourceRevision ||
        !facts.stepId ||
        facts.saving ||
        facts.dirty ||
        facts.conflict
      )
        return;
      value.pending = {
        requestId: `web-xmp-${randomUuid()}`,
        expectedRecipeVersion: facts.recipeRevision,
        expectedSourceRevision: facts.sourceRevision,
      };
    }
    const body = value.pending;
    value.submitting = true;
    value.generation++;
    value.state = "submitting";
    value.note = "Exporting the current Edit State…";
    render(id);
    const uncertain = () => {
      value.state = "outcome-unknown";
      value.note =
        "The XMP export outcome is unknown. Check result to repeat the same request.";
    };
    try {
      const response = await fetcher(
        `/api/photos/${encodeURIComponent(id)}/edit-state-exports`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        },
      );
      if (!response.ok) {
        const note = await failureNote(response);
        if (response.status >= 500) uncertain();
        else {
          delete value.pending;
          value.state = "failed";
          value.note = note;
        }
      } else {
        const artifact = parseXmpArtifact(
          await response.json().catch(() => undefined),
          id,
        );
        if (
          !artifact ||
          artifact.recipeRevision !== body.expectedRecipeVersion ||
          artifact.sourceRevision !== body.expectedSourceRevision
        )
          uncertain();
        else {
          delete value.pending;
          value.artifact = artifact;
          value.state = "succeeded";
          value.note =
            "Ready to download. XMP contains parameters, not rendered pixels.";
        }
      }
    } catch {
      uncertain();
    } finally {
      value.submitting = false;
      render(id);
    }
  };
  const download = async (id: string): Promise<void> => {
    if (!dependencies.owns(id)) return;
    const value = session(id);
    const artifact = value.artifact;
    const stamp = scope;
    if (!artifact || expired(artifact.expiresAt)) return;
    const note = (text: string): void => {
      if (owns(id, stamp)) {
        value.note = text;
        render(id);
      }
    };
    try {
      const response = await fetcher(
        `/api/photos/${encodeURIComponent(id)}/edit-state-exports/${encodeURIComponent(artifact.exportId)}/artifact`,
        { priority: "high" },
      );
      if (!response.ok) {
        note(await failureNote(response));
        return;
      }
      const expected: Record<string, string> = {
        "slipstream-artifact-export-id": artifact.exportId,
        "slipstream-artifact-target": "edit-state-xmp",
        "slipstream-artifact-filename": artifact.filename,
        "slipstream-artifact-content-type": artifact.contentType,
        "slipstream-artifact-byte-length": String(artifact.byteLength),
        "slipstream-artifact-sha256": artifact.sha256,
        "slipstream-artifact-created-at": artifact.createdAt,
        "slipstream-artifact-expires-at": artifact.expiresAt,
      };
      if (
        response.headers.get("content-type") !== artifact.contentType ||
        !Object.entries(expected).every(
          ([name, expectedValue]) =>
            response.headers.get(name) === expectedValue,
        )
      ) {
        note(
          "The downloaded file did not match this XMP artifact and was discarded.",
        );
        return;
      }
      const blob = await response.blob();
      if (
        blob.size !== artifact.byteLength ||
        (await blobSha256Hex(blob)) !== artifact.sha256
      ) {
        note(
          "The downloaded file was incomplete or changed and was discarded.",
        );
        return;
      }
      if (!owns(id, stamp)) return;
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = artifact.filename;
      anchor.click();
      window.setTimeout(() => URL.revokeObjectURL(url), 0);
      note("Download started.");
    } catch {
      note("The download could not complete. Try again.");
    }
  };
  return {
    open(id: string) {
      photo = id;
      scope++;
      session(id);
      void refresh(id);
    },
    refresh,
    leave() {
      scope++;
      photo = undefined;
    },
    view(id: string): WorkspaceOutputsView {
      const value = session(id);
      const facts = dependencies.facts(id);
      return {
        xmp: {
          state: value.state,
          note:
            value.artifact && expired(value.artifact.expiresAt)
              ? "This file has expired. Export again."
              : value.note,
          artifact: value.artifact,
          isStale: Boolean(
            value.artifact &&
              (value.artifact.recipeRevision !== facts?.recipeRevision ||
                value.artifact.sourceRevision !== facts?.sourceRevision),
          ),
          canSubmit:
            !value.submitting &&
            (Boolean(value.pending) ||
              Boolean(
                facts?.recipeRevision &&
                  facts.sourceRevision &&
                  facts.stepId &&
                  !facts.saving &&
                  !facts.dirty &&
                  !facts.conflict,
              )),
          canDownload: Boolean(
            value.artifact && !expired(value.artifact.expiresAt),
          ),
        },
      };
    },
    submit,
    download,
    unresolved: (id: string): boolean => Boolean(sessions.get(id)?.pending),
  };
}
