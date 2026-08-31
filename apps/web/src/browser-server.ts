import { spawn, type ChildProcess } from "node:child_process";
import { once } from "node:events";
import { access } from "node:fs/promises";
import { createServer } from "node:net";
import { join, resolve } from "node:path";

export type BrowserServer = Readonly<{
  url: string;
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

  for (let attempt = 1; attempt <= maxStartupAttempts; attempt += 1) {
    const port = await availablePort();
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
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    const errors: string[] = [];
    child.stderr.on("data", (chunk: Buffer) => {
      if (errors.join("").length < 8_192) errors.push(chunk.toString());
    });
    const url = `http://127.0.0.1:${port}`;
    try {
      await waitForReady(child, url, errors);
      let closing: Promise<void> | undefined;
      return {
        url,
        close() {
          closing ??= stop(child);
          return closing;
        },
      };
    } catch (error) {
      await stop(child);
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
