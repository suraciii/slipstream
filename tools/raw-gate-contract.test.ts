import { expect, test } from "bun:test";

import {
  rawGateListCommand,
  rawGatePlan,
  validateRawGateDiscovery,
} from "../scripts/raw-gate-plan.ts";

const repositoryRoot = new URL("../", import.meta.url);

async function runRawGate(
  sample: string | undefined,
  args: readonly string[] = [],
): Promise<{ output: string; exitCode: number }> {
  const environment = { ...process.env };
  if (sample === undefined) delete environment.SLIPSTREAM_RAW_SAMPLE;
  else environment.SLIPSTREAM_RAW_SAMPLE = sample;
  const command = ["bun", "run", "test:raw"];
  if (args.length > 0) command.push("--", ...args);
  const child = Bun.spawn(command, {
    cwd: repositoryRoot.pathname,
    env: environment,
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  return { output: `${stdout}${stderr}`, exitCode };
}

test("test:raw fails clearly before scenarios when the sample is absent", async () => {
  const result = await runRawGate(undefined);
  expect(result.exitCode).not.toBe(0);
  expect(result.output).toContain("SLIPSTREAM_RAW_SAMPLE is required");
});

test("test:raw rejects a path that is not a regular file", async () => {
  const result = await runRawGate("/slipstream/raw-sample-does-not-exist");
  expect(result.exitCode).not.toBe(0);
  expect(result.output).toContain(
    "SLIPSTREAM_RAW_SAMPLE must identify a readable regular file",
  );
});

test("the RAW plan keeps the exact serial native, service, and browser gate", () => {
  expect(rawGatePlan).toEqual([
    {
      kind: "cargo",
      label: "native and service scenarios",
      command: [
        "cargo",
        "test",
        "--workspace",
        "--locked",
        "--",
        "--ignored",
        "--test-threads=1",
      ],
      expectedTests: [
        "confinement::tests::sony_original_remains_unchanged",
        "native::tests::sony_embedded_preview_is_largest_usable_candidate_and_original_is_unchanged",
        "preview::tests::raw_photo_previews_from_embedded_jpeg_independent_of_sibling_jpeg",
        "preview::tests::sony_opt_in_service_uses_largest_embedded_candidate_without_mutating_original",
      ],
    },
    {
      kind: "playwright",
      label: "real-camera browser scenarios",
      command: [
        "bun",
        "run",
        "test:browser",
        "--",
        "--grep",
        "real-camera",
        "--workers=1",
      ],
      expectedTests: [
        "real-camera: shows matching JPEG then RAW embedded JPEG through the mobile production Review Session",
      ],
    },
  ]);

  expect(rawGateListCommand(rawGatePlan[0]!)).toEqual([
    "cargo",
    "test",
    "--workspace",
    "--locked",
    "--",
    "--ignored",
    "--test-threads=1",
    "--list",
  ]);
  expect(rawGateListCommand(rawGatePlan[1]!)).toEqual([
    "bun",
    "run",
    "test:browser",
    "--",
    "--grep",
    "real-camera",
    "--workers=1",
    "--list",
  ]);
});

test("discovery parses exactly four native/service and one browser scenario", () => {
  const [native, browser] = rawGatePlan;
  const nativeOutput = native!.expectedTests
    .map((name) => `${name}: test`)
    .join("\n");
  const browserOutput = browser!.expectedTests
    .map((name) => `browser-review.browser-test.ts:1:1 \u203a ${name}`)
    .join("\n");

  expect(
    validateRawGateDiscovery(native!, { output: nativeOutput, exitCode: 0 }),
  ).toEqual({ label: native!.label, tests: native!.expectedTests });
  expect(
    validateRawGateDiscovery(browser!, {
      output: browserOutput,
      exitCode: 0,
    }),
  ).toEqual({ label: browser!.label, tests: browser!.expectedTests });
});

test("discovery fails when its command fails", () => {
  expect(() =>
    validateRawGateDiscovery(rawGatePlan[0]!, {
      output: "cargo failed",
      exitCode: 17,
    }),
  ).toThrow(
    "native and service scenarios discovery failed with exit code 17: cargo failed",
  );
});

test("discovery fails instead of accepting zero intended scenarios", () => {
  expect(() =>
    validateRawGateDiscovery(rawGatePlan[0]!, { output: "", exitCode: 0 }),
  ).toThrow("discovery expected 4 tests but found 0");
});

test("discovery fails instead of accepting an extra scenario", () => {
  const step = rawGatePlan[0]!;
  const output = [...step.expectedTests, "unexpected::real_camera_test"]
    .map((name) => `${name}: test`)
    .join("\n");

  expect(() => validateRawGateDiscovery(step, { output, exitCode: 0 })).toThrow(
    "native and service scenarios discovery expected 4 tests but found 5",
  );
});
