import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { once } from "node:events";
import { access, readFile, writeFile, mkdir, chmod } from "node:fs/promises";
import { createHash, randomBytes } from "node:crypto";
import {
  createServer as createHttpsServer,
  request as httpsRequest,
} from "node:https";
import { request as httpRequest } from "node:http";
import { readFileSync } from "node:fs";
import { createServer } from "node:net";
import { join, resolve } from "node:path";

export type BrowserServer = Readonly<{
  url: string;
  token: string;
  tokenFile: string;
  close(): Promise<void>;
}>;

type BrowserServerOptions = Readonly<{
  base: string;
  root: string;
}>;

const startupTimeoutMs = 60_000;
const maxStartupAttempts = 3;

export async function startBrowserServer({
  base,
  root,
}: BrowserServerOptions): Promise<BrowserServer> {
  const webRoot = resolve(process.env.SLIPSTREAM_WEB_ROOT ?? "apps/web/dist");
  const binary = resolve(
    process.env.SLIPSTREAM_SERVER_BINARY ?? "target/debug/slipstream-server",
  );
  await access(binary);
  await access(join(webRoot, "index.html"));

  const token = randomBytes(32).toString("base64url");
  const tokenFile = join(base, "access-token");
  await writeFile(tokenFile, token, { mode: 0o600 });
  await mkdir(join(base, "state"), { recursive: true, mode: 0o700 });
  const records = JSON.stringify({
    credential: {
      digest: Array.from(createHash("sha256").update(token).digest()),
      generation: randomBytes(32).toString("base64url"),
    },
    sessions: [],
  });
  execFileSync("python3", [
    "-c",
    "import sqlite3,sys,os; p=sys.argv[1]; c=sqlite3.connect(p); c.execute('CREATE TABLE IF NOT EXISTS access (id INTEGER PRIMARY KEY CHECK(id=1), records TEXT NOT NULL)'); c.execute('INSERT OR REPLACE INTO access VALUES(1, ?)', (sys.argv[2],)); c.commit(); c.close(); os.chmod(p,0o600)",
    join(base, "state/access.sqlite"),
    records,
  ]);
  await chmod(join(base, "state/access.sqlite"), 0o600);
  for (let attempt = 1; attempt <= maxStartupAttempts; attempt += 1) {
    const port = await availablePort();
    const backendUrl = `http://127.0.0.1:${port}`;
    const proxy = createHttpsServer(
      {
        key: await readFile(resolve("tools/test-tls/server-key.pem")),
        cert: await readFile(resolve("tools/test-tls/server-cert.pem")),
      },
      (incoming, outgoing) => {
        const upstream = httpRequest(
          `${backendUrl}${incoming.url ?? "/"}`,
          { method: incoming.method, headers: incoming.headers },
          (response) => {
            outgoing.writeHead(response.statusCode ?? 502, response.headers);
            response.pipe(outgoing);
          },
        );
        upstream.on("error", () => {
          outgoing.writeHead(502);
          outgoing.end();
        });
        incoming.pipe(upstream);
      },
    );
    await new Promise<void>((done) => proxy.listen(0, "127.0.0.1", done));
    const address = proxy.address();
    if (!address || typeof address === "string")
      throw new Error("HTTPS proxy address unavailable");
    const url = `https://127.0.0.1:${address.port}`;
    const child = spawn(binary, [], {
      cwd: process.cwd(),
      env: {
        ...process.env,
        SLIPSTREAM_LIBRARY_ROOT: root,
        SLIPSTREAM_STATE_DIRECTORY: join(base, "state"),
        SLIPSTREAM_DATABASE_BASENAME: "library.sqlite",
        SLIPSTREAM_CACHE_DIRECTORY: join(base, "cache"),
        SLIPSTREAM_WEB_ROOT: webRoot,
        SLIPSTREAM_HOST: "127.0.0.1",
        SLIPSTREAM_PORT: String(port),
        SLIPSTREAM_PUBLIC_ORIGIN: url,
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    const errors: string[] = [];
    child.stderr.on("data", (chunk: Buffer) => {
      if (errors.join("").length < 8_192) errors.push(chunk.toString());
    });
    try {
      await waitForReady(child, backendUrl, errors);
      fixtureTokens.set(url, token);
      let closing: Promise<void> | undefined;
      return {
        url,
        token,
        tokenFile,
        close() {
          closing ??= (async () => {
            fixtureTokens.delete(url);
            await stop(child);
            proxy.closeAllConnections();
            await new Promise<void>((done) => proxy.close(() => done()));
          })();
          return closing;
        },
      };
    } catch (error) {
      await stop(child);
      proxy.closeAllConnections();
      await new Promise<void>((done) => proxy.close(() => done()));
      if (
        attempt === maxStartupAttempts ||
        !retryableStartupFailure(error, errors)
      )
        throw error;
    }
  }
  throw new Error("Rust browser server startup attempts exhausted");
}

function retryableStartupFailure(error: unknown, errors: string[]): boolean {
  const message = error instanceof Error ? error.message : String(error);
  if (message.includes("did not become ready")) return true;
  if (!message.includes("exited before readiness")) return false;
  return /address already in use|address in use|eaddrinuse|bind/i.test(
    errors.join(""),
  );
}

async function waitForReady(
  child: ChildProcess,
  url: string,
  errors: string[],
): Promise<void> {
  const deadline = Date.now() + startupTimeoutMs;
  while (Date.now() < deadline) {
    if (child.exitCode !== null || child.signalCode !== null) {
      throw new Error(
        `Rust browser server exited before readiness${errors.length ? `: ${errors.join("")}` : ""}`,
      );
    }
    try {
      const response = await fetch(`${url}/healthz`);
      if (response.ok && (await response.text()) === '{"status":"ok"}') return;
    } catch {
      // The listener is not ready yet.
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(
    `Rust browser server did not become ready${errors.length ? `: ${errors.join("")}` : ""}`,
  );
}

async function stop(child: ChildProcess): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const exited = once(child, "exit");
  child.kill("SIGTERM");
  await exited;
}

async function availablePort(): Promise<number> {
  const probe = createServer();
  await new Promise<void>((resolve, reject) => {
    probe.once("error", reject);
    probe.listen(0, "127.0.0.1", resolve);
  });
  const address = probe.address();
  const port = typeof address === "object" && address ? address.port : 0;
  await new Promise<void>((resolve, reject) =>
    probe.close((error) => (error ? reject(error) : resolve())),
  );
  return port;
}

// Fixture requests use real TLS verification and explicit Bearer credentials.
const fixtureTokens = new Map<string, string>();
export async function fixtureFetch(
  input: RequestInfo | URL,
  init: RequestInit = {},
): Promise<Response> {
  const url = new URL(input instanceof Request ? input.url : String(input));
  const token = fixtureTokens.get(url.origin);
  if (!token) throw new Error("Unknown authenticated fixture origin");
  const headers = new Headers(init.headers);
  headers.set("Authorization", `Bearer ${token}`);
  const outgoingHeaders: Record<string, string> = {};
  headers.forEach((value, key) => {
    outgoingHeaders[key] = value;
  });
  return new Promise((resolveResponse, reject) => {
    const request = httpsRequest(
      url,
      {
        method: init.method ?? "GET",
        headers: outgoingHeaders,
        ca: readFileSync(resolve("tools/test-tls/cert.pem")),
      },
      (incoming) => {
        const chunks: Buffer[] = [];
        incoming.on("data", (chunk: Buffer) => chunks.push(chunk));
        incoming.on("end", () => {
          const responseHeaders = new Headers();
          for (const [key, value] of Object.entries(incoming.headers)) {
            if (value !== undefined)
              responseHeaders.set(
                key,
                Array.isArray(value) ? value.join(", ") : value,
              );
          }
          const status = incoming.statusCode ?? 500;
          resolveResponse(
            new Response(
              [204, 304].includes(status) ? null : Buffer.concat(chunks),
              { status, headers: responseHeaders },
            ),
          );
        });
        incoming.on("error", reject);
      },
    );
    request.on("error", reject);
    if (init.body) request.write(init.body);
    request.end();
  });
}
