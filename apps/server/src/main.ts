import { isAbsolute } from "node:path";

import { startServer } from "./http-server.js";

export type StartupOptions = Readonly<{
  libraryRoot: string;
  stateDirectory: string;
  databaseBasename: string;
  cacheDirectory: string;
  host: string;
  port: number;
}>;

export const defaultNetworkOptions = { host: "127.0.0.1", port: 3000 } as const;

export function startupOptions(environment: NodeJS.ProcessEnv): StartupOptions {
  const libraryRoot = requiredAbsolute(
    environment.SLIPSTREAM_LIBRARY_ROOT,
    "SLIPSTREAM_LIBRARY_ROOT",
  );
  const stateDirectory = requiredAbsolute(
    environment.SLIPSTREAM_STATE_DIRECTORY,
    "SLIPSTREAM_STATE_DIRECTORY",
  );
  const cacheDirectory = requiredAbsolute(
    environment.SLIPSTREAM_CACHE_DIRECTORY,
    "SLIPSTREAM_CACHE_DIRECTORY",
  );
  const databaseBasename =
    environment.SLIPSTREAM_DATABASE_BASENAME ?? "library.sqlite";
  const host = environment.SLIPSTREAM_HOST ?? defaultNetworkOptions.host;
  const portText =
    environment.SLIPSTREAM_PORT ?? String(defaultNetworkOptions.port);
  if (!/^\d+$/.test(portText))
    throw new Error("SLIPSTREAM_PORT must be an integer");
  return {
    libraryRoot,
    stateDirectory,
    cacheDirectory,
    databaseBasename,
    host,
    port: Number(portText),
  };
}

async function main(): Promise<void> {
  const server = await startServer(startupOptions(process.env));
  console.log(`Slipstream listening at ${server.url}`);
  let stopping = false;
  const stop = async () => {
    if (stopping) return;
    stopping = true;
    await server.close();
  };
  process.once("SIGINT", () => void stop());
  process.once("SIGTERM", () => void stop());
}

function requiredAbsolute(value: string | undefined, name: string): string {
  if (!value || !isAbsolute(value))
    throw new Error(`${name} must be an absolute path`);
  return value;
}

if (import.meta.main) {
  main().catch((error: unknown) => {
    console.error(
      error instanceof Error ? error.message : "Slipstream startup failed",
    );
    process.exitCode = 1;
  });
}
