import type { BrowserFetch } from "./browser-fetch.js";
import { isRecord } from "./guards.js";

export type JsonObject = Record<string, unknown>;
export type WorkspaceWindow<T> = {
  items: T[];
  total: number;
  nextCursor: string | null;
};
export type WorkspaceFile = JsonObject & {
  artifactId: string;
  revision: string | null;
  relativePath: string;
  kind: string;
  state: string;
  byteLength: number | null;
  imageContract: JsonObject | null;
  imageFacts?: JsonObject | null;
};
export type Workspace = JsonObject & {
  workspaceId: string;
  photoId: string;
  path?: string;
  inputPath?: string | null;
  inputArtifactId: string | null;
  inputRevision?: string | null;
  inputAvailability?: string;
  inputMatchesObservedSource?: boolean | null;
  observedSourceRevision?: string | null;
  execution?: JsonObject;
  files?: WorkspaceWindow<WorkspaceFile>;
};

export class WorkspaceApiError extends Error {
  constructor(
    public code: string,
    message: string,
    public effect: string,
    public details: JsonObject,
  ) {
    super(message);
  }
}

export async function workspaceRequest(
  fetcher: BrowserFetch,
  path: string,
  body?: unknown,
  method = body === undefined ? "GET" : "POST",
): Promise<JsonObject> {
  let response: Response;
  try {
    response = await fetcher(path, {
      method,
      ...(body === undefined
        ? {}
        : {
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify(body),
          }),
    });
  } catch {
    throw new WorkspaceApiError(
      body === undefined ? "transport_failed" : "outcome_unknown",
      "Connection lost. Check the Workspace before retrying.",
      body === undefined ? "none" : "unknown",
      {},
    );
  }
  let value: unknown;
  try {
    value = await response.json();
  } catch {
    throw new WorkspaceApiError(
      body === undefined ? "invalid_response" : "outcome_unknown",
      "The service response could not be read. Check the Workspace before retrying.",
      body === undefined ? "none" : "unknown",
      {},
    );
  }
  if (!response.ok) {
    const error = isRecord(value) && isRecord(value.error) ? value.error : {};
    throw new WorkspaceApiError(
      typeof error.code === "string" ? error.code : "service_failed",
      typeof error.message === "string"
        ? error.message
        : `Workspace request failed (${response.status}).`,
      typeof error.effect === "string"
        ? error.effect
        : body === undefined
          ? "none"
          : "unknown",
      isRecord(error.details) ? error.details : {},
    );
  }
  if (!isRecord(value))
    throw new WorkspaceApiError(
      "invalid_response",
      "Expected a structured Workspace result.",
      body === undefined ? "none" : "unknown",
      {},
    );
  return value;
}

export function parseWindow<T>(
  value: unknown,
  parse: (value: unknown) => T,
): WorkspaceWindow<T> {
  if (
    !isRecord(value) ||
    !Array.isArray(value.items) ||
    !Number.isInteger(value.total) ||
    !(value.nextCursor === null || typeof value.nextCursor === "string")
  )
    throw new WorkspaceApiError(
      "schema_error",
      "The Workspace returned an invalid bounded file list.",
      "none",
      {},
    );
  return {
    items: value.items.map(parse),
    total: value.total as number,
    nextCursor: value.nextCursor,
  };
}

export function parseWorkspaceFile(value: unknown): WorkspaceFile {
  if (
    !isRecord(value) ||
    typeof value.artifactId !== "string" ||
    !(value.revision === null || typeof value.revision === "string") ||
    typeof value.relativePath !== "string" ||
    typeof value.kind !== "string" ||
    typeof value.state !== "string" ||
    !(value.byteLength === null || typeof value.byteLength === "number")
  )
    throw new WorkspaceApiError(
      "schema_error",
      "The Workspace returned invalid file facts.",
      "none",
      {},
    );
  return value as WorkspaceFile;
}

export function parseWorkspace(value: unknown): Workspace {
  if (
    !isRecord(value) ||
    typeof value.workspaceId !== "string" ||
    value.workspaceId.length === 0 ||
    typeof value.photoId !== "string" ||
    !(
      value.inputArtifactId === null ||
      typeof value.inputArtifactId === "string"
    )
  )
    throw new WorkspaceApiError(
      "schema_error",
      "The service returned an invalid Workspace identity.",
      "none",
      {},
    );
  return {
    ...value,
    ...(value.files === undefined
      ? {}
      : { files: parseWindow(value.files, parseWorkspaceFile) }),
  } as Workspace;
}

export const workspacePath = (workspaceId: string) =>
  `/api/workspaces/${encodeURIComponent(workspaceId)}`;

export function workspaceFilesPath(
  workspaceId: string,
  cursor: string | null,
): string {
  return cursor === null
    ? `${workspacePath(workspaceId)}/files?limit=60`
    : `/api/workspace-files?cursor=${encodeURIComponent(cursor)}`;
}

export function workspaceFilePath(
  workspaceId: string,
  artifactId: string,
): string {
  return `${workspacePath(workspaceId)}/files/${encodeURIComponent(artifactId)}`;
}

export function fileBytesUrl(workspaceId: string, file: WorkspaceFile): string {
  if (file.state !== "ready" || !file.revision)
    throw new WorkspaceApiError(
      "file_unavailable",
      "Only a ready checked file revision can be downloaded.",
      "none",
      {},
    );
  return `${workspaceFilePath(workspaceId, file.artifactId)}/bytes?revision=${encodeURIComponent(file.revision)}`;
}

/// Checks that a Download URL still names the ready revision before the
/// browser navigates to it. A refusal reports the service's current readiness
/// without mutating workspace state.
export async function checkWorkspaceDownload(
  fetcher: BrowserFetch,
  url: string,
): Promise<void> {
  let response: Response;
  try {
    response = await fetcher(url, {
      headers: { Range: "bytes=0-0" },
      cache: "no-store",
    });
  } catch {
    throw new WorkspaceApiError(
      "transport_failed",
      "Connection lost. Check the Workspace before downloading again.",
      "none",
      {},
    );
  }
  if (response.ok) {
    try {
      await response.body?.cancel();
    } catch {
      // The response is still a successful admission check.
    }
    return;
  }
  let value: unknown;
  try {
    value = await response.json();
  } catch {
    throw new WorkspaceApiError(
      "invalid_response",
      "The service refused the download without a readable reason.",
      "none",
      {},
    );
  }
  const error = isRecord(value) && isRecord(value.error) ? value.error : {};
  throw new WorkspaceApiError(
    typeof error.code === "string" ? error.code : "download_failed",
    typeof error.message === "string"
      ? error.message
      : "The selected file is no longer ready.",
    "none",
    isRecord(error.details) ? error.details : {},
  );
}
