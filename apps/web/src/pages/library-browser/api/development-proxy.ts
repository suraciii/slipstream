import type { BrowserFetch } from "../model/access-session.js";
import { isRecord } from "./guards.js";

export type DevelopmentProxyState = "absent" | "building" | "current" | "stale";
export type DevelopmentProxyFacts = Readonly<{
  proxyId: string;
  sourceRevision: string;
  sourceSize: number;
  sourceSha256: string;
  sourceProfileId: string;
  pipelineVersion: string;
  longEdge: number;
  width: number;
  height: number;
  qualityLimit: string;
  byteLength: number;
  sha256: string;
  createdAt: number;
}>;
export type DevelopmentProxyStatus = Readonly<{
  photoId: string;
  state: DevelopmentProxyState;
  proxy: DevelopmentProxyFacts | null;
  failure: string | null;
}>;

const hex64 = (value: unknown): value is string =>
  typeof value === "string" && /^[0-9a-f]{64}$/.test(value);

const parseProxy = (value: unknown): DevelopmentProxyFacts | null => {
  if (!isRecord(value)) return null;
  const numberKeys = [
    "sourceSize",
    "longEdge",
    "width",
    "height",
    "byteLength",
  ] as const;
  if (
    typeof value.proxyId !== "string" ||
    typeof value.sourceRevision !== "string" ||
    !hex64(value.sourceSha256) ||
    typeof value.sourceProfileId !== "string" ||
    typeof value.pipelineVersion !== "string" ||
    typeof value.qualityLimit !== "string" ||
    !hex64(value.sha256) ||
    typeof value.createdAt !== "number" ||
    !Number.isFinite(value.createdAt) ||
    numberKeys.some(
      (key) => typeof value[key] !== "number" || !Number.isFinite(value[key]),
    )
  )
    return null;
  return Object.freeze({
    proxyId: value.proxyId,
    sourceRevision: value.sourceRevision,
    sourceSize: value.sourceSize as number,
    sourceSha256: value.sourceSha256,
    sourceProfileId: value.sourceProfileId,
    pipelineVersion: value.pipelineVersion,
    longEdge: value.longEdge as number,
    width: value.width as number,
    height: value.height as number,
    qualityLimit: value.qualityLimit,
    byteLength: value.byteLength as number,
    sha256: value.sha256,
    createdAt: value.createdAt,
  });
};

export const parseDevelopmentProxyStatus = (
  value: unknown,
  photoId: string,
): DevelopmentProxyStatus | undefined => {
  if (!isRecord(value) || value.photoId !== photoId) return undefined;
  const state = value.state;
  if (
    state !== "absent" &&
    state !== "building" &&
    state !== "current" &&
    state !== "stale"
  )
    return undefined;
  const proxy = value.proxy === null ? null : parseProxy(value.proxy);
  if (state === "current" && proxy === null) return undefined;
  const failureValue = value.failure;
  const failure =
    failureValue === null || failureValue === undefined
      ? null
      : isRecord(failureValue) && typeof failureValue.reason === "string"
        ? failureValue.reason
        : null;
  return Object.freeze({ photoId, state, proxy, failure });
};

const proxyPath = (photoId: string): string =>
  `/api/photos/${encodeURIComponent(photoId)}/development-proxy`;

export type ProxyOutcome =
  | Readonly<{ kind: "ok"; status: DevelopmentProxyStatus }>
  | Readonly<{ kind: "failed"; message: string }>;

const read = async (
  response: Response,
  photoId: string,
): Promise<ProxyOutcome> => {
  const body: unknown = await response.json().catch(() => undefined);
  const status = parseDevelopmentProxyStatus(body, photoId);
  return status
    ? { kind: "ok", status }
    : {
        kind: "failed",
        message: `Development Proxy response was invalid (${response.status}).`,
      };
};

export const fetchDevelopmentProxy = async (
  fetcher: BrowserFetch,
  photoId: string,
  signal?: AbortSignal,
): Promise<ProxyOutcome> => {
  try {
    return await read(
      await fetcher(proxyPath(photoId), {
        priority: "high",
        ...(signal ? { signal } : {}),
      }),
      photoId,
    );
  } catch {
    return {
      kind: "failed",
      message: "The Development Proxy status could not be read.",
    };
  }
};

export const createDevelopmentProxy = async (
  fetcher: BrowserFetch,
  photoId: string,
  expectedSourceRevision: string,
  signal?: AbortSignal,
): Promise<ProxyOutcome> => {
  try {
    return await read(
      await fetcher(proxyPath(photoId), {
        method: "POST",
        priority: "high",
        ...(signal ? { signal } : {}),
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ expectedSourceRevision }),
      }),
      photoId,
    );
  } catch {
    return {
      kind: "failed",
      message: "The Development Proxy could not be created.",
    };
  }
};

export const removeDevelopmentProxy = async (
  fetcher: BrowserFetch,
  photoId: string,
  signal?: AbortSignal,
): Promise<ProxyOutcome> => {
  try {
    return await read(
      await fetcher(proxyPath(photoId), {
        method: "DELETE",
        priority: "high",
        ...(signal ? { signal } : {}),
      }),
      photoId,
    );
  } catch {
    return {
      kind: "failed",
      message: "The Development Proxy could not be removed.",
    };
  }
};
