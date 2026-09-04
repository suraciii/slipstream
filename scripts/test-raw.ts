import { stat } from "node:fs/promises";
import { resolve } from "node:path";

import { discoverRawGate, rawGatePlan } from "./raw-gate-plan.ts";

const repositoryRoot = resolve(import.meta.dirname, "..");
const rawArguments = process.argv.slice(2);

if (
  rawArguments.length > 0 &&
  (rawArguments.length !== 1 || rawArguments[0] !== "--list")
) {
  console.error("test:raw accepts only --list as an optional argument");
  process.exit(1);
}

async function discoverOrExit() {
  try {
    return await discoverRawGate(repositoryRoot);
  } catch (error) {
    console.error(
      `test:raw discovery failed: ${
        error instanceof Error ? error.message : String(error)
      }`,
    );
    process.exit(1);
  }
}

if (rawArguments[0] === "--list") {
  for (const discovery of await discoverOrExit()) {
    console.log(`${discovery.label}: ${discovery.tests.length} discovered`);
    for (const test of discovery.tests) console.log(`  ${test}`);
  }
  process.exit(0);
}

const configuredSample = process.env.SLIPSTREAM_RAW_SAMPLE?.trim();

if (!configuredSample) {
  console.error(
    "SLIPSTREAM_RAW_SAMPLE is required for test:raw; set it to the configured Sony Original File path",
  );
  process.exit(1);
}

const samplePath = resolve(configuredSample);
try {
  const metadata = await stat(samplePath);
  if (!metadata.isFile()) throw new Error("not a regular file");
} catch {
  console.error(
    `SLIPSTREAM_RAW_SAMPLE must identify a readable regular file: ${samplePath}`,
  );
  process.exit(1);
}

const environment = { ...process.env, SLIPSTREAM_RAW_SAMPLE: samplePath };

await discoverOrExit();

async function run(label: string, command: readonly string[]): Promise<void> {
  const child = Bun.spawn([...command], {
    cwd: repositoryRoot,
    env: environment,
    stdout: "inherit",
    stderr: "inherit",
  });
  const exitCode = await child.exited;
  if (exitCode !== 0) {
    console.error(`test:raw ${label} failed with exit code ${exitCode}`);
    process.exit(exitCode || 1);
  }
}

for (const step of rawGatePlan) await run(step.label, step.command);
