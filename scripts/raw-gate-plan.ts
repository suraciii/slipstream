export type RawGateStep = Readonly<{
  kind: "cargo" | "playwright";
  label: string;
  command: readonly string[];
  expectedTests: readonly string[];
}>;

const nativeAndServiceTests = [
  "confinement::tests::sony_original_remains_unchanged",
  "native::tests::sony_embedded_preview_is_largest_usable_candidate_and_original_is_unchanged",
  "preview::tests::raw_photo_previews_from_embedded_jpeg_independent_of_sibling_jpeg",
  "preview::tests::sony_opt_in_service_uses_largest_embedded_candidate_without_mutating_original",
] as const;

const browserTests = [
  "real-camera: shows matching JPEG then RAW embedded JPEG through the mobile production Review Session",
] as const;

export const rawGatePlan: readonly RawGateStep[] = [
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
      // The display derivative keeps an opt-in test of its own, but it needs a
      // Development TIFF (`SLIPSTREAM_DEVELOPMENT_TIFF_SAMPLE`) rather than the
      // camera RAW sample this gate provides, so it is neither run nor counted
      // here.
      "--skip",
      "development_tiff_derivative_handles_a_real_artifact",
    ],
    expectedTests: nativeAndServiceTests,
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
    expectedTests: browserTests,
  },
];

export type RawGateDiscovery = Readonly<{
  label: string;
  tests: readonly string[];
}>;

export type RawGateCommandResult = Readonly<{
  output: string;
  exitCode: number;
}>;

export function rawGateListCommand(step: RawGateStep): string[] {
  return [...step.command, "--list"];
}

async function capture(
  command: readonly string[],
  cwd: string,
): Promise<RawGateCommandResult> {
  const child = Bun.spawn([...command], {
    cwd,
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  return { output: `${stdout}\n${stderr}`, exitCode };
}

function cargoTests(output: string): string[] {
  return output.split(/\r?\n/).flatMap((line) => {
    const match = line.match(/^\s*(.+): test$/);
    return match ? [match[1]!.trim()] : [];
  });
}

function playwrightTests(output: string): string[] {
  const marker = " \u203a ";
  return output.split(/\r?\n/).flatMap((line) => {
    const markerIndex = line.indexOf(marker);
    return markerIndex < 0
      ? []
      : [line.slice(markerIndex + marker.length).trim()];
  });
}

function discoveredTests(step: RawGateStep, output: string): string[] {
  return step.kind === "cargo" ? cargoTests(output) : playwrightTests(output);
}

function sameTests(actual: readonly string[], expected: readonly string[]) {
  const actualSorted = [...actual].sort();
  const expectedSorted = [...expected].sort();
  return (
    actualSorted.length === expectedSorted.length &&
    actualSorted.every((name, index) => name === expectedSorted[index])
  );
}

export function validateRawGateDiscovery(
  step: RawGateStep,
  result: RawGateCommandResult,
): RawGateDiscovery {
  if (result.exitCode !== 0) {
    throw new Error(
      `${step.label} discovery failed with exit code ${result.exitCode}: ${result.output.trim()}`,
    );
  }
  const tests = discoveredTests(step, result.output);
  if (!sameTests(tests, step.expectedTests)) {
    throw new Error(
      `${step.label} discovery expected ${step.expectedTests.length} tests but found ${tests.length}: ${tests.join(", ")}`,
    );
  }
  return { label: step.label, tests };
}

export async function discoverRawGate(
  cwd: string,
): Promise<RawGateDiscovery[]> {
  const discoveries: RawGateDiscovery[] = [];
  for (const step of rawGatePlan) {
    const result = await capture(rawGateListCommand(step), cwd);
    discoveries.push(validateRawGateDiscovery(step, result));
  }
  return discoveries;
}
