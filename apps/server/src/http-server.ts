import { readFile, stat } from "node:fs/promises";
import { extname, join, normalize, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import { serve } from "@hono/node-server";
import { Hono } from "hono";

import {
  SlipstreamApplication,
  etag,
  type ApplicationConfig,
} from "./application.js";

export type ServerConfig = ApplicationConfig & Readonly<{ webRoot?: string }>;
export type RunningServer = Readonly<{
  url: string;
  close(): Promise<void>;
}>;

const defaultWebRoot = fileURLToPath(
  new URL("../../web/dist/", import.meta.url),
);
const maximumHeaderBytes = 16 * 1024;

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
  app.post("/api/scan", async (context) => {
    const origin = context.req.header("origin");
    if (origin && !sameOrigin(origin, context.req.raw.url))
      return context.json({ error: "Cross-origin mutation rejected" }, 403);
    return context.json(await application.rescan());
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
  app.onError(
    () =>
      new Response(JSON.stringify({ error: "Request failed" }), {
        status: 500,
        headers: { "Content-Type": "application/json" },
      }),
  );
  app.notFound((context) => context.json({ error: "Not found" }, 404));
  return app;
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
