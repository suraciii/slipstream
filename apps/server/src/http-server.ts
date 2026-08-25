import { readFile, stat } from "node:fs/promises";
import { extname, join, normalize, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import { serve } from "@hono/node-server";
import { Hono, type Context } from "hono";

import {
  SlipstreamApplication,
  etag,
  type ApplicationConfig,
} from "./application.js";
import { MutationError } from "./library/photo-library.js";

export type ServerConfig = ApplicationConfig & Readonly<{ webRoot?: string }>;
export type RunningServer = Readonly<{
  url: string;
  close(): Promise<void>;
}>;

const defaultWebRoot = fileURLToPath(
  new URL("../../web/dist/", import.meta.url),
);
const maximumHeaderBytes = 16 * 1024;
const maximumMutationBodyBytes = 64 * 1024;

export async function startServer(
  config: ServerConfig,
): Promise<RunningServer> {
  const application = await SlipstreamApplication.open(config);
  const webRoot = resolve(config.webRoot ?? defaultWebRoot);
  const app = createHttpApp(application, webRoot);
  let server: ReturnType<typeof serve>;
  try {
    server = serve({
      fetch: app.fetch,
      hostname: config.host,
      port: config.port,
    });
  } catch (error) {
    await application.shutdown();
    throw error;
  }
  try {
    await new Promise<void>((resolveReady, reject) => {
      const ready = () => {
        cleanup();
        resolveReady();
      };
      const failed = (error: Error) => {
        cleanup();
        reject(error);
      };
      const cleanup = () => {
        server.off("listening", ready);
        server.off("error", failed);
      };
      server.once("listening", ready);
      server.once("error", failed);
    });
  } catch (error) {
    if (server.listening)
      await new Promise<void>((resolveClose) =>
        server.close(() => resolveClose()),
      );
    await application.shutdown();
    throw error;
  }
  const address = server.address();
  const port =
    typeof address === "object" && address ? address.port : config.port;
  let closing: Promise<void> | undefined;
  return {
    url: `http://${config.host}:${port}`,
    close() {
      closing ??= (async () => {
        await new Promise<void>((resolveClose, reject) =>
          server.close((error) => (error ? reject(error) : resolveClose())),
        );
        await application.shutdown();
      })();
      return closing;
    },
  };
}

export function createHttpApp(
  application: SlipstreamApplication,
  webRoot: string,
): Hono {
  const app = new Hono();
  app.use("*", async (context, next) => {
    const headerBytes = [...context.req.raw.headers].reduce(
      (sum, [name, value]) => sum + name.length + value.length,
      0,
    );
    if (headerBytes > maximumHeaderBytes)
      return context.json({ error: "Request headers are too large" }, 431);
    if (!["GET", "HEAD", "POST"].includes(context.req.method))
      return context.json({ error: "Method not allowed" }, 405);
    await next();
  });
  app.get("/api/photos", (context) => context.json(application.listPhotos()));
  app.get("/api/photo-sets", async (context) =>
    context.json({ photoSets: await application.listPhotoSets() }),
  );
  app.post("/api/scan", async (context) => {
    if (!mutationAllowed(context.req.header("origin"), context.req.raw.url))
      return context.json({ error: "Cross-origin mutation rejected" }, 403);
    return context.json(await application.rescan());
  });
  app.post("/api/photo-sets", async (context) => {
    if (!mutationAllowed(context.req.header("origin"), context.req.raw.url))
      return context.json({ error: "Cross-origin mutation rejected" }, 403);
    const body = await jsonObject(context.req.raw);
    const name = validName(body?.name);
    if (!name) return context.json({ error: "Invalid Photo Set name" }, 400);
    return mutate(context, () => application.createPhotoSet(name));
  });
  app.post("/api/photo-sets/:id/rename", async (context) => {
    if (!mutationAllowed(context.req.header("origin"), context.req.raw.url))
      return context.json({ error: "Cross-origin mutation rejected" }, 403);
    const body = await jsonObject(context.req.raw);
    const name = validName(body?.name);
    if (!validId(context.req.param("id")) || !name)
      return context.json({ error: "Invalid Photo Set mutation" }, 400);
    return mutate(context, () =>
      application.renamePhotoSet(context.req.param("id"), name),
    );
  });
  app.post("/api/photo-sets/:id/delete", async (context) => {
    if (!mutationAllowed(context.req.header("origin"), context.req.raw.url))
      return context.json({ error: "Cross-origin mutation rejected" }, 403);
    if (!validId(context.req.param("id")))
      return context.json({ error: "Invalid Photo Set" }, 400);
    return mutate(context, () =>
      application.deletePhotoSet(context.req.param("id")),
    );
  });
  app.post("/api/photo-sets/:id/members", async (context) => {
    if (!mutationAllowed(context.req.header("origin"), context.req.raw.url))
      return context.json({ error: "Cross-origin mutation rejected" }, 403);
    const body = await jsonObject(context.req.raw);
    const ids = validIds(body?.photoIds);
    if (!validId(context.req.param("id")) || !ids)
      return context.json({ error: "Invalid membership batch" }, 400);
    return mutate(context, () =>
      application.addPhotoSetMembers(context.req.param("id"), ids),
    );
  });
  app.post("/api/photo-sets/:id/members/remove", async (context) => {
    if (!mutationAllowed(context.req.header("origin"), context.req.raw.url))
      return context.json({ error: "Cross-origin mutation rejected" }, 403);
    const body = await jsonObject(context.req.raw);
    const photoId = body?.photoId;
    if (!validId(context.req.param("id")) || !validId(photoId))
      return context.json({ error: "Invalid Photo Set member" }, 400);
    return mutate(context, () =>
      application.removePhotoSetMember(context.req.param("id"), photoId),
    );
  });
  app.post("/api/photo-sets/:id/order", async (context) => {
    if (!mutationAllowed(context.req.header("origin"), context.req.raw.url))
      return context.json({ error: "Cross-origin mutation rejected" }, 403);
    const body = await jsonObject(context.req.raw);
    const ids = validIds(body?.photoIds);
    if (!validId(context.req.param("id")) || !ids)
      return context.json({ error: "Invalid Photo Set order" }, 400);
    return mutate(context, () =>
      application.reorderPhotoSet(context.req.param("id"), ids),
    );
  });
  app.post("/api/photo-sets/:id/progress", async (context) => {
    if (!mutationAllowed(context.req.header("origin"), context.req.raw.url))
      return context.json({ error: "Cross-origin mutation rejected" }, 403);
    const body = await jsonObject(context.req.raw);
    const photoId = body?.photoId;
    if (!validId(context.req.param("id")) || !validId(photoId))
      return context.json({ error: "Invalid review progress" }, 400);
    return mutate(context, () =>
      application.setReviewProgress(context.req.param("id"), photoId),
    );
  });
  app.post("/api/photos/:id/state", async (context) => {
    if (!mutationAllowed(context.req.header("origin"), context.req.raw.url))
      return context.json({ error: "Cross-origin mutation rejected" }, 403);
    const body = await jsonObject(context.req.raw);
    const field = body?.field;
    const rawValue = body?.value;
    const rawExpected = body?.expectedCurrent;
    const selectionValue = selectionState(rawValue);
    const ratingValue = rating(rawValue);
    const selection =
      field === "selectionState" && selectionValue !== undefined;
    const ratingMutation = field === "rating" && ratingValue !== undefined;
    const expectedCurrent =
      rawExpected === undefined
        ? undefined
        : selection
          ? selectionState(rawExpected)
          : rating(rawExpected);
    const expectedValid =
      rawExpected === undefined || expectedCurrent !== undefined;
    const value = selection ? selectionValue : ratingValue;
    if (
      !validId(context.req.param("id")) ||
      (!selection && !ratingMutation) ||
      !expectedValid ||
      (body?.photoSetId !== undefined && !validId(body.photoSetId))
    )
      return context.json({ error: "Invalid Photo state mutation" }, 400);
    if (value === undefined)
      return context.json({ error: "Invalid Photo state mutation" }, 400);
    try {
      const result = await application.mutatePhotoState({
        photoId: context.req.param("id"),
        field: selection ? "selectionState" : "rating",
        value,
        ...(expectedCurrent !== undefined ? { expectedCurrent } : {}),
        ...(body?.photoSetId ? { photoSetId: body.photoSetId } : {}),
      });
      return context.json(result);
    } catch (error) {
      return mutationErrorResponse(context, error);
    }
  });
  app.get("/api/photos/:id/preview", async (context) => {
    const result = await application.preview(context.req.param("id"));
    return context.json(result, result.state === "ready" ? 200 : 404);
  });
  app.get("/api/derivatives/:photoId/:filename", async (context) => {
    const filename = context.req.param("filename");
    const match = /^([a-f0-9]{64})\.jpg$/.exec(filename);
    if (!match) return context.json({ error: "Derivative not found" }, 404);
    const result = await application.derivative(
      context.req.param("photoId"),
      match[1]!,
    );
    if (!result) return context.json({ error: "Derivative not found" }, 404);
    const entityTag = etag(result.cacheKey);
    if (context.req.header("if-none-match") === entityTag)
      return new Response(null, { status: 304, headers: { ETag: entityTag } });
    const facts = await stat(result.cachePath).catch(() => undefined);
    if (!facts?.isFile())
      return context.json({ error: "Derivative not found" }, 404);
    const bytes = await readFile(result.cachePath).catch(() => undefined);
    if (!bytes) return context.json({ error: "Derivative not found" }, 404);
    return new Response(bytes, {
      headers: {
        "Content-Type": "image/jpeg",
        "Content-Length": String(facts.size),
        "Cache-Control": "public, max-age=31536000, immutable",
        ETag: entityTag,
        "X-Content-Type-Options": "nosniff",
      },
    });
  });
  app.get("*", async (context) => {
    if (context.req.path.startsWith("/api/"))
      return context.json({ error: "Not found" }, 404);
    const requested =
      context.req.path === "/" ? "index.html" : context.req.path.slice(1);
    const safe = safeWebPath(webRoot, requested);
    const path =
      safe && (await stat(safe).catch(() => undefined))?.isFile()
        ? safe
        : join(webRoot, "index.html");
    const bytes = await readFile(path).catch(() => undefined);
    if (!bytes) return context.text("Web application is not built", 503);
    return new Response(bytes, {
      headers: {
        "Content-Type": contentType(path),
        "Cache-Control": path.endsWith("index.html")
          ? "no-cache"
          : "public, max-age=31536000, immutable",
        "X-Content-Type-Options": "nosniff",
      },
    });
  });
  app.onError((error, context) => {
    if (error instanceof BodyError)
      return context.json({ error: error.message }, error.status);
    return context.json({ error: "Request failed" }, 500);
  });
  app.notFound((context) => context.json({ error: "Not found" }, 404));
  return app;
}

class BodyError extends Error {
  constructor(
    readonly status: 400 | 413,
    message: string,
  ) {
    super(message);
  }
}
async function jsonObject(request: Request): Promise<Record<string, unknown>> {
  const declared = request.headers.get("content-length");
  if (declared !== null) {
    const length = Number(declared);
    if (!Number.isSafeInteger(length) || length < 0)
      throw new BodyError(400, "Invalid request body");
    if (length > maximumMutationBodyBytes)
      throw new BodyError(413, "Request body is too large");
  }
  const reader = request.body?.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  if (reader) {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      total += value.byteLength;
      if (total > maximumMutationBodyBytes) {
        await reader.cancel();
        throw new BodyError(413, "Request body is too large");
      }
      chunks.push(value);
    }
  }
  const bytes = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  try {
    const value: unknown = JSON.parse(new TextDecoder().decode(bytes));
    if (!value || typeof value !== "object" || Array.isArray(value))
      throw new Error();
    return value as Record<string, unknown>;
  } catch {
    throw new BodyError(400, "Invalid JSON body");
  }
}
function mutationAllowed(
  origin: string | undefined,
  requestUrl: string,
): boolean {
  return !origin || sameOrigin(origin, requestUrl);
}
function validId(value: unknown): value is string {
  return typeof value === "string" && /^[a-f0-9-]{36,64}$/.test(value);
}
function selectionState(
  value: unknown,
): "undecided" | "selected" | "rejected" | undefined {
  return value === "undecided" || value === "selected" || value === "rejected"
    ? value
    : undefined;
}
function rating(value: unknown): number | undefined {
  return Number.isInteger(value) &&
    (value as number) >= 0 &&
    (value as number) <= 5
    ? (value as number)
    : undefined;
}
function validName(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const name = value.trim();
  return name.length >= 1 && name.length <= 120 ? name : undefined;
}
function validIds(value: unknown): string[] | undefined {
  return Array.isArray(value) &&
    value.length <= 100 &&
    value.every(validId) &&
    new Set(value).size === value.length
    ? value
    : undefined;
}
async function mutate(context: Context, operation: () => Promise<unknown>) {
  try {
    return context.json({ photoSets: await operation() });
  } catch (error) {
    return mutationErrorResponse(context, error);
  }
}
function mutationErrorResponse(context: Context, error: unknown) {
  if (error instanceof MutationError) {
    if (error.kind === "not-found")
      return context.json({ error: "Mutation target not found" }, 404);
    if (error.kind === "conflict")
      return context.json(
        { error: "Mutation conflicts with current state" },
        409,
      );
    return context.json({ error: "Mutation could not be persisted" }, 503);
  }
  return context.json({ error: "Mutation could not be persisted" }, 503);
}

function sameOrigin(origin: string, requestUrl: string): boolean {
  try {
    const supplied = new URL(origin);
    const request = new URL(requestUrl);
    return supplied.origin === request.origin;
  } catch {
    return false;
  }
}

function safeWebPath(root: string, requested: string): string | undefined {
  if (requested.includes("\0") || requested.includes("\\")) return undefined;
  const path = resolve(root, normalize(requested));
  return path === root || path.startsWith(root + sep) ? path : undefined;
}

function contentType(path: string): string {
  switch (extname(path)) {
    case ".html":
      return "text/html; charset=utf-8";
    case ".js":
      return "text/javascript; charset=utf-8";
    case ".css":
      return "text/css; charset=utf-8";
    case ".svg":
      return "image/svg+xml";
    default:
      return "application/octet-stream";
  }
}
