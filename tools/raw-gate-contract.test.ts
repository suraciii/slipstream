import { expect, test } from "bun:test";
import { cp, mkdtemp, rm, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

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

test("browser discovery lists the real camera scenario without production artifacts or a build", async () => {
  const root = await mkdtemp(join(tmpdir(), "slipstream-raw-discovery-"));
  try {
    await cp(
      new URL("../apps/web/src", import.meta.url),
      join(root, "apps/web/src"),
      { recursive: true },
    );
    await cp(
      new URL("../playwright.config.ts", import.meta.url),
      join(root, "playwright.config.ts"),
    );
    await symlink(
      new URL("../node_modules", import.meta.url).pathname,
      join(root, "node_modules"),
      "dir",
    );
    const manifest = (await Bun.file(
      new URL("../package.json", import.meta.url),
    ).json()) as { scripts: Record<string, string> };
    manifest.scripts.build = `bun -e 'await Bun.write("production-build-invoked", "unexpected build"); process.exit(29)'`;
    await Bun.write(join(root, "package.json"), JSON.stringify(manifest));

    const browser = rawGatePlan.find((step) => step.kind === "playwright")!;
    const child = Bun.spawn(rawGateListCommand(browser), {
      cwd: root,
      env: {
        ...process.env,
        SLIPSTREAM_RAW_SAMPLE: undefined,
        SLIPSTREAM_SERVER_BINARY: join(root, "target/debug/slipstream-server"),
        SLIPSTREAM_CLI_BINARY: join(root, "target/debug/slipstream"),
        SLIPSTREAM_WEB_ROOT: join(root, "apps/web/dist"),
        PLAYWRIGHT_BROWSERS_PATH: join(root, "missing-browsers"),
        PLAYWRIGHT_CHROMIUM_EXECUTABLE: join(root, "missing-chromium"),
      },
      stdout: "pipe",
      stderr: "pipe",
    });
    const [stdout, stderr, exitCode] = await Promise.all([
      new Response(child.stdout).text(),
      new Response(child.stderr).text(),
      child.exited,
    ]);

    expect(
      await Bun.file(join(root, "production-build-invoked")).exists(),
    ).toBe(false);
    expect(
      validateRawGateDiscovery(browser, {
        output: `${stdout}\n${stderr}`,
        exitCode,
      }).tests,
    ).toEqual([
      "real-camera: shows matching JPEG then RAW embedded JPEG through the mobile production Review Session",
    ]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}, 30_000);

test("discovery parses exactly four native/service and one browser scenario", () => {
  const [native, browser] = rawGatePlan;
  expect(native!.expectedTests).toHaveLength(4);
  expect(browser!.expectedTests).toHaveLength(1);
  const nativeOutput = native!.expectedTests
    .map((name) => `${name}: test`)
    .join("\n");
  const browserOutput = browser!.expectedTests
    .map((name) => `browser-smoke.browser-test.ts:1:1 \u203a ${name}`)
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
  for (const step of rawGatePlan) {
    expect(() =>
      validateRawGateDiscovery(step, {
        output: "discovery command failed",
        exitCode: 17,
      }),
    ).toThrow(
      `${step.label} discovery failed with exit code 17: discovery command failed`,
    );
  }
});

test("discovery fails instead of accepting zero intended scenarios", () => {
  for (const step of rawGatePlan) {
    expect(() =>
      validateRawGateDiscovery(step, { output: "", exitCode: 0 }),
    ).toThrow(
      `discovery expected ${step.expectedTests.length} tests but found 0`,
    );
  }
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
