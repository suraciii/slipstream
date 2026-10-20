import type { BrowserFetch } from "./browser-fetch.js";
import { isRecord } from "./guards.js";

export type JsonObject = Record<string, unknown>;
export type Capability = {
  read: boolean;
  write: boolean;
  execute: boolean;
  reason: string | null;
  recovery: string | null;
};
export type Window<T> = {
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
};
export type Workspace = JsonObject & {
  workspaceId: string;
  photoId: string;
  inputArtifactId: string | null;
  observedSourceRevision?: string | null;
  edits: (JsonObject & { editId: string; tool: string; name: string })[];
};
export type EditInstance = JsonObject & {
  instanceId: string;
  moduleId: string;
  name: string;
  enabled: boolean;
  order: number;
  parameters?: JsonObject;
  initialized?: JsonObject;
  blend?: JsonObject;
  availability: Capability;
};
export type EditView = JsonObject & {
  revision: string;
  baselineRevision: string;
  stateToken: string;
  editId: string;
  bundleId: string;
  schemaVersion: string;
  availability: Capability;
  instances: Window<EditInstance>;
  masks: Window<JsonObject>;
  actions: Window<JsonObject>;
  output: {
    settings: JsonObject;
    schema: JsonObject;
    availability: Capability;
  };
  frames: JsonObject[];
};
export type ToolJob = JsonObject & {
  requestId: string;
  status: string;
  revision?: string;
};
export function parseToolJob(value: unknown): ToolJob {
  if (
    !isRecord(value) ||
    typeof value.requestId !== "string" ||
    typeof value.status !== "string"
  )
    throw new WorkspaceApiError(
      "schema_error",
      "The tool returned an invalid job identity or status.",
      "none",
      {},
    );
  const { revision, ...fields } = value;
  if (
    revision !== undefined &&
    revision !== null &&
    typeof revision !== "string"
  )
    throw new WorkspaceApiError(
      "schema_error",
      "The tool returned an invalid captured revision.",
      "none",
      {},
    );
  return {
    ...fields,
    requestId: value.requestId,
    status: value.status,
    ...(typeof revision === "string" ? { revision } : {}),
  };
}
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
      "Connection lost. Check or replay this exact request.",
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
      "The service response could not be read. Check or replay this exact request.",
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
        : `Request failed (${response.status}).`,
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
      "Expected a structured service result.",
      body === undefined ? "none" : "unknown",
      {},
    );
  return value;
}
export function parseWindow<T>(
  value: unknown,
  parse: (value: unknown) => T,
): Window<T> {
  if (
    !isRecord(value) ||
    !Array.isArray(value.items) ||
    !Number.isInteger(value.total) ||
    !(value.nextCursor === null || typeof value.nextCursor === "string")
  )
    throw new WorkspaceApiError(
      "schema_error",
      "The tool returned an invalid bounded window.",
      "none",
      {},
    );
  return {
    items: value.items.map(parse),
    total: value.total as number,
    nextCursor: value.nextCursor,
  };
}
export function parseEdit(value: JsonObject): EditView {
  for (const key of [
    "revision",
    "baselineRevision",
    "stateToken",
    "editId",
    "bundleId",
    "schemaVersion",
  ])
    if (typeof value[key] !== "string")
      throw new WorkspaceApiError(
        "schema_error",
        `Missing tool field ${key}. Inspect the saved state and installed tool schema.`,
        "none",
        { field: key },
      );
  if (
    !isRecord(value.availability) ||
    !isRecord(value.output) ||
    !isRecord(value.output.settings) ||
    !isRecord(value.output.schema) ||
    !Array.isArray(value.frames)
  )
    throw new WorkspaceApiError(
      "schema_error",
      "Invalid tool capability/output schema.",
      "none",
      {},
    );
  const instance = (item: unknown): EditInstance => {
    if (
      !isRecord(item) ||
      typeof item.instanceId !== "string" ||
      typeof item.moduleId !== "string" ||
      typeof item.enabled !== "boolean" ||
      typeof item.order !== "number"
    )
      throw new WorkspaceApiError(
        "schema_error",
        "Invalid operation instance.",
        "none",
        {},
      );
    return item as EditInstance;
  };
  const object = (item: unknown): JsonObject => {
    if (!isRecord(item))
      throw new WorkspaceApiError(
        "schema_error",
        "Invalid tool object.",
        "none",
        {},
      );
    return item;
  };
  return {
    ...value,
    instances: parseWindow(value.instances, instance),
    masks: parseWindow(value.masks, object),
    actions: parseWindow(value.actions, object),
  } as EditView;
}
export function parseFile(value: unknown): WorkspaceFile {
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
      "Invalid workspace file evidence.",
      "none",
      {},
    );
  return value as WorkspaceFile;
}
export const toolPath = (photoId: string) =>
  `/api/photos/${encodeURIComponent(photoId)}/darktable`;
export const workspacePath = (workspaceId: string) =>
  `/api/workspaces/${encodeURIComponent(workspaceId)}`;
export function fileBytesUrl(workspaceId: string, file: WorkspaceFile): string {
  if (file.state !== "ready" || !file.revision)
    throw new WorkspaceApiError(
      "file_unavailable",
      "Only a ready checked file revision can be downloaded.",
      "none",
      {},
    );
  return `${workspacePath(workspaceId)}/files/${encodeURIComponent(file.artifactId)}/bytes?revision=${encodeURIComponent(file.revision)}`;
}
export function toolCommand(
  fetcher: BrowserFetch,
  photoId: string,
  editId: string | null,
  operation: string,
  args: JsonObject = {},
): Promise<JsonObject> {
  return workspaceRequest(fetcher, `${toolPath(photoId)}/commands`, {
    operation,
    editId,
    arguments: args,
  });
}
export function requestIdentity(): string {
  return crypto.randomUUID();
}
