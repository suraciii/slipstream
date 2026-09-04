import { expect, test } from "bun:test";
import { createHash } from "node:crypto";
import {
  chmod,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  readlink,
  realpath,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = fileURLToPath(new URL("../", import.meta.url));
const composeEntryPoint = join(repositoryRoot, "scripts", "compose");
const composeFile = join(repositoryRoot, "compose.yaml");
const originalBytes = "Original bytes must not change";

type StorageRole = "library" | "state" | "cache";

const storageEnvironmentKeys: Record<StorageRole, string> = {
  library: "SLIPSTREAM_LIBRARY_ROOT",
  state: "SLIPSTREAM_STATE_DIRECTORY",
  cache: "SLIPSTREAM_CACHE_DIRECTORY",
};

interface Fixture {
  root: string;
  environmentFile: string;
  composeArguments: string;
  composeConfiguration: string;
  composeEnvironment: string;
  composeSources: string;
  dockerCalls: string;
  findmntData: string;
  path: string;
}

interface Topology {
  sources: Record<StorageRole, string>;
}

interface CommandResult {
  exitCode: number;
  stderr: string;
}

async function dockerComposeConfig(
  environmentFile: string,
): Promise<Record<string, unknown>> {
  const environment = { ...process.env };
  for (const key of Object.keys(environment)) {
    if (
      key.startsWith("SLIPSTREAM_") ||
      key.startsWith("COMPOSE_") ||
      key.startsWith("DOCKER_")
    ) {
      delete environment[key];
    }
  }

  let child: ReturnType<typeof Bun.spawn>;
  try {
    child = Bun.spawn(
      [
        "docker",
        "compose",
        "--env-file",
        environmentFile,
        "-f",
        composeFile,
        "config",
        "--format",
        "json",
      ],
      {
        cwd: repositoryRoot,
        env: environment,
        stderr: "pipe",
        stdout: "pipe",
      },
    );
  } catch (error) {
    throw new Error(
      "Docker Compose is required for the Compose configuration contract test",
      { cause: error },
    );
  }

  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  if (exitCode !== 0) {
    throw new Error(
      `Docker Compose config failed with exit code ${exitCode}: ${stderr.trim()}`,
    );
  }
  try {
    return JSON.parse(stdout) as Record<string, unknown>;
  } catch (error) {
    throw new Error("Docker Compose config returned invalid JSON", {
      cause: error,
    });
  }
}

interface MountCoordinate {
  endpoint: string;
  target: string;
  fsroot: string;
  device: string;
}

interface OriginalEvidence {
  contentHash: string;
  metadata: readonly string[];
}

async function fixture(): Promise<Fixture> {
  const root = await mkdtemp(join(tmpdir(), "slipstream-compose-storage-"));
  const bin = join(root, "bin");
  const composeArguments = join(root, "compose-arguments");
  const composeConfiguration = join(root, "compose-configuration");
  const composeEnvironment = join(root, "compose-environment");
  const composeSources = join(root, "compose-sources");
  const dockerCalls = join(root, "docker-calls");
  const findmntData = join(root, "findmnt-data");
  const environmentFile = join(root, "slipstream.env");
  await mkdir(bin);
  const docker = join(bin, "docker");
  await writeFile(
    docker,
    [
      "#!/usr/bin/env bash",
      "set -euo pipefail",
      "printf 'called\\n' > \"$FAKE_DOCKER_CALLS\"",
      'if [[ "$1" == "context" && "$2" == "inspect" && "$3" == "default" ]]; then',
      "  printf '%s\\n' \"${FAKE_DOCKER_CONTEXT_ENDPOINT:-unix:///var/run/docker.sock}\"",
      "  exit 0",
      "fi",
      'if [[ "$1" == "--context" && "$2" == "default" && "$3" == "compose" ]]; then',
      "  shift 3",
      '  printf \'%s\\n\' "$@" > "$FAKE_DOCKER_COMPOSE_ARGUMENTS"',
      '  printf \'%s\\n\' "${SLIPSTREAM_IMAGE-}" "${SLIPSTREAM_VCS_REF-}" "${SLIPSTREAM_BIND_ADDRESS-}" "${SLIPSTREAM_PORT-}" "${SLIPSTREAM_PUBLIC_ORIGIN-}" "${SLIPSTREAM_DATABASE_BASENAME-}" > "$FAKE_DOCKER_COMPOSE_CONFIGURATION"',
      '  printf \'%s\\n\' "${COMPOSE_FILE-}" "${COMPOSE_ENV_FILES-}" "${COMPOSE_PROFILES-}" "${COMPOSE_PROJECT_NAME-}" > "$FAKE_DOCKER_COMPOSE_ENVIRONMENT"',
      '  printf \'%s\\n\' "${SLIPSTREAM_LIBRARY_ROOT-}" "${SLIPSTREAM_STATE_DIRECTORY-}" "${SLIPSTREAM_CACHE_DIRECTORY-}" > "$FAKE_DOCKER_COMPOSE_SOURCES"',
      "  exit 0",
      "fi",
      "printf 'unexpected docker command\\n' >&2",
      "exit 90",
      "",
    ].join("\n"),
  );
  await chmod(docker, 0o755);
  const findmnt = join(bin, "findmnt");
  await writeFile(
    findmnt,
    [
      "#!/usr/bin/env bash",
      "set -euo pipefail",
      'original_arguments=("$@")',
      'if [[ "${FAKE_FINDMNT_FAIL:-}" == "1" ]]; then',
      "  exit 1",
      "fi",
      'endpoint=""',
      "noheadings=false",
      "raw=false",
      "submounts=false",
      "print_raw() {",
      "  local value=$1",
      "  value=${value//\\/\\x5c}",
      "  value=${value// /\\x20}",
      '  printf "%s" "$value"',
      "}",
      "print_row() {",
      '  print_raw "$1"',
      "  printf ' '",
      '  print_raw "$2"',
      "  printf ' %s\\n' \"$3\"",
      "}",
      'while [[ "$#" -gt 0 ]]; do',
      '  case "$1" in',
      "    --noheadings) noheadings=true; shift ;;",
      "    --raw) raw=true; shift ;;",
      "    --submounts) submounts=true; shift ;;",
      "    --uniq) shift ;;",
      "    -T) endpoint=$2; shift 2 ;;",
      "    -o) shift 2 ;;",
      "    *) shift ;;",
      "  esac",
      "done",
      '[[ "$noheadings" == true && "$raw" == true && "$submounts" == true ]] || exit 92',
      'case "${FAKE_FINDMNT_RAW_OVERRIDE:-}" in',
      "  control) printf '%s\\n' '/unsafe\\x0a'; exit 0 ;;",
      "  malformed) printf '%s\\n' '/unsafe\\xZZ'; exit 0 ;;",
      "  byte) printf '/unsafe\\377 / 7:42\\n'; exit 0 ;;",
      "esac",
      'if [[ -s "${FAKE_FINDMNT_DATA:-}" ]]; then',
      "  found=false",
      "  while IFS=$'\\t' read -r mapped_endpoint target fsroot device || [[ -n \"$mapped_endpoint\" ]]; do",
      '    if [[ "$mapped_endpoint" == "$endpoint" ]]; then',
      "      found=true",
      '      print_row "$target" "$fsroot" "$device"',
      "    fi",
      '  done < "$FAKE_FINDMNT_DATA"',
      '  if [[ "$found" == true ]]; then',
      '    if [[ "${FAKE_FINDMNT_TRAILING_EMPTY:-}" == "1" ]]; then',
      '      printf "\\n"',
      "    fi",
      "    exit 0",
      "  fi",
      "fi",
      'exec env PATH="$FAKE_SYSTEM_PATH" findmnt "${original_arguments[@]}"',
      "",
    ].join("\n"),
  );
  await chmod(findmnt, 0o755);
  return {
    root,
    environmentFile,
    composeArguments,
    composeConfiguration,
    composeEnvironment,
    composeSources,
    dockerCalls,
    findmntData,
    path: `${bin}:${process.env.PATH ?? ""}`,
  };
}

async function writeMountCoordinates(
  target: Fixture,
  coordinates: readonly MountCoordinate[],
): Promise<void> {
  await writeFile(
    target.findmntData,
    `${coordinates
      .map(({ endpoint, target: mountTarget, fsroot, device }) =>
        [endpoint, mountTarget, fsroot, device].join("\t"),
      )
      .join("\n")}\n`,
  );
}

function directMountCoordinates(
  sources: Record<StorageRole, string>,
  target = "/",
  fsroot = "/",
  device = "7:42",
): Record<StorageRole, MountCoordinate> {
  return {
    library: {
      endpoint: sources.library,
      target,
      fsroot,
      device,
    },
    state: {
      endpoint: sources.state,
      target,
      fsroot,
      device,
    },
    cache: {
      endpoint: sources.cache,
      target,
      fsroot,
      device,
    },
  };
}

async function topology(target: Fixture): Promise<Topology> {
  const root = await mkdtemp(join(target.root, "topology-"));
  const sources = {
    library: join(root, "originals"),
    state: join(root, "state"),
    cache: join(root, "cache"),
  };
  await Promise.all(Object.values(sources).map((path) => mkdir(path)));
  return { sources };
}

async function writeEnvironment(
  target: Fixture,
  sources: Record<StorageRole, string>,
): Promise<void> {
  await writeFile(
    target.environmentFile,
    [
      "SLIPSTREAM_IMAGE=slipstream:from-environment-file",
      "SLIPSTREAM_VCS_REF=environment-file-ref",
      "SLIPSTREAM_BIND_ADDRESS=127.0.0.2",
      "SLIPSTREAM_PORT=3100",
      "SLIPSTREAM_PUBLIC_ORIGIN=https://environment-file.invalid",
      "SLIPSTREAM_DATABASE_BASENAME=environment-file.sqlite",
      `SLIPSTREAM_LIBRARY_ROOT=${sources.library}`,
      `SLIPSTREAM_STATE_DIRECTORY=${sources.state}`,
      `SLIPSTREAM_CACHE_DIRECTORY=${sources.cache}`,
    ].join("\n"),
  );
}

async function runCompose(
  target: Fixture,
  command: readonly string[],
  options: {
    environmentFile?: string;
    environment?: Record<string, string>;
  } = {},
): Promise<CommandResult> {
  const environment = { ...process.env };
  delete environment.DOCKER_CONTEXT;
  delete environment.DOCKER_HOST;
  const child = Bun.spawn(
    [
      composeEntryPoint,
      "--env-file",
      options.environmentFile ?? target.environmentFile,
      ...command,
    ],
    {
      cwd: repositoryRoot,
      env: {
        ...environment,
        PATH: target.path,
        FAKE_DOCKER_COMPOSE_ARGUMENTS: target.composeArguments,
        FAKE_DOCKER_COMPOSE_CONFIGURATION: target.composeConfiguration,
        FAKE_DOCKER_COMPOSE_ENVIRONMENT: target.composeEnvironment,
        FAKE_DOCKER_COMPOSE_SOURCES: target.composeSources,
        FAKE_DOCKER_CALLS: target.dockerCalls,
        FAKE_FINDMNT_DATA: target.findmntData,
        FAKE_SYSTEM_PATH: process.env.PATH ?? "",
        ...options.environment,
      },
      stderr: "pipe",
      stdout: "pipe",
    },
  );
  const stderr = await new Response(child.stderr).text();
  return { exitCode: await child.exited, stderr };
}

async function originalSnapshot(root: string): Promise<readonly string[]> {
  const entries = await readdir(root, { withFileTypes: true });
  const snapshot: string[] = [];
  for (const entry of entries.sort((left, right) =>
    left.name.localeCompare(right.name),
  )) {
    const path = join(root, entry.name);
    if (entry.isDirectory()) {
      snapshot.push(`${entry.name}/`);
      for (const descendant of await originalSnapshot(path)) {
        snapshot.push(`${entry.name}/${descendant}`);
      }
    } else if (entry.isFile()) {
      snapshot.push(
        `${entry.name}:${(await readFile(path)).toString("base64")}`,
      );
    } else if (entry.isSymbolicLink()) {
      snapshot.push(`${entry.name}@${await readlink(path)}`);
    }
  }
  return snapshot;
}

async function originalEvidence(root: string): Promise<OriginalEvidence> {
  const contentHash = createHash("sha256");
  const metadata: string[] = [];

  async function visit(path: string, relativePath: string): Promise<void> {
    const information = await lstat(path, { bigint: true });
    const type = information.isDirectory()
      ? "directory"
      : information.isFile()
        ? "file"
        : information.isSymbolicLink()
          ? "symlink"
          : "other";
    const relative = relativePath || ".";
    const linkTarget = information.isSymbolicLink() ? await readlink(path) : "";
    metadata.push(
      [
        relative,
        type,
        information.mode.toString(),
        information.uid.toString(),
        information.gid.toString(),
        information.size.toString(),
        information.mtimeNs.toString(),
        information.ctimeNs.toString(),
        linkTarget,
      ].join("\0"),
    );
    if (information.isFile()) {
      contentHash.update(await readFile(path));
    } else if (information.isSymbolicLink()) {
      contentHash.update(linkTarget);
    } else if (information.isDirectory()) {
      const entries = (await readdir(path, { withFileTypes: true })).sort(
        (left, right) => left.name.localeCompare(right.name),
      );
      for (const entry of entries) {
        await visit(
          join(path, entry.name),
          relativePath ? `${relativePath}/${entry.name}` : entry.name,
        );
      }
    }
  }

  await visit(root, "");
  return { contentHash: contentHash.digest("hex"), metadata };
}

function validEnvironmentBytes(sources: Record<StorageRole, string>): Buffer {
  return Buffer.from(
    [
      `SLIPSTREAM_LIBRARY_ROOT=${sources.library}`,
      `SLIPSTREAM_STATE_DIRECTORY=${sources.state}`,
      `SLIPSTREAM_CACHE_DIRECTORY=${sources.cache}`,
    ].join("\n"),
  );
}

function nulInValueEnvironmentBytes(
  sources: Record<StorageRole, string>,
  role: StorageRole,
): Buffer {
  const chunks: Buffer[] = [];
  for (const candidate of ["library", "state", "cache"] as const) {
    const value = Buffer.from(sources[candidate]);
    const nulValue =
      candidate === role
        ? Buffer.concat([
            value.subarray(0, Math.ceil(value.length / 2)),
            Buffer.from([0]),
            value.subarray(Math.ceil(value.length / 2)),
          ])
        : value;
    chunks.push(
      Buffer.from(`${storageEnvironmentKeys[candidate]}=`),
      nulValue,
      Buffer.from("\n"),
    );
  }
  return Buffer.concat(chunks);
}

function nulInKeyEnvironmentBytes(
  sources: Record<StorageRole, string>,
): Buffer {
  return Buffer.concat([
    Buffer.from(`SLIPSTREAM_LIBRARY_ROOT=${sources.library}\n`),
    Buffer.from("SLIPSTREAM_STATE_DIRECT"),
    Buffer.from([0]),
    Buffer.from(
      `ORY=${sources.state}\nSLIPSTREAM_CACHE_DIRECTORY=${sources.cache}`,
    ),
  ]);
}

async function expectNulEnvironmentRejected(
  target: Fixture,
  sources: Record<StorageRole, string>,
  environment: Buffer,
  command: readonly string[],
): Promise<void> {
  await writeFile(join(sources.library, "preserved.ARW"), originalBytes);
  await writeFile(target.environmentFile, environment);
  const before = await originalEvidence(sources.library);

  const result = await runCompose(target, command);

  expect(result.exitCode).toBe(2);
  expect(result.stderr).toContain(
    "environment file must not contain NUL bytes",
  );
  expect(await Bun.file(target.composeArguments).exists()).toBeFalse();
  expect(await Bun.file(target.dockerCalls).exists()).toBeFalse();
  expect(await originalEvidence(sources.library)).toEqual(before);
}

async function prepareInvalidTopology(
  target: Fixture,
  left: StorageRole,
  right: StorageRole,
  kind: "equal" | "forward" | "reverse" | "alias",
): Promise<Topology> {
  const result = await topology(target);
  const { sources } = result;
  if (kind === "equal") {
    sources[right] = sources[left];
  } else if (kind === "forward") {
    const nested = join(sources[left], `${right}-nested`);
    await mkdir(nested);
    sources[right] = nested;
  } else if (kind === "reverse") {
    const parent = join(target.root, `${left}-${right}-parent`);
    const child = join(parent, `${left}-nested`);
    await mkdir(child, { recursive: true });
    sources[left] = child;
    sources[right] = parent;
  } else {
    const alias = join(target.root, `${left}-${right}-alias`);
    await symlink(sources[left], alias);
    sources[right] = alias;
  }
  await writeFile(join(sources.library, "preserved.ARW"), originalBytes);
  return result;
}

async function removeFixture(target: Fixture): Promise<void> {
  await rm(target.root, { force: true, recursive: true });
}

for (const role of ["library", "state", "cache"] as const) {
  for (const kind of ["missing", "non-directory"] as const) {
    test(`startup rejects ${kind} ${role} storage before Docker`, async () => {
      const target = await fixture();
      try {
        const layout = await topology(target);
        await writeFile(
          join(layout.sources.library, "preserved.ARW"),
          originalBytes,
        );
        const before = await originalEvidence(layout.sources.library);
        const invalidPath = join(target.root, `${role}-${kind}`);
        if (kind === "non-directory") await writeFile(invalidPath, "file");
        await writeEnvironment(target, {
          ...layout.sources,
          [role]: invalidPath,
        });

        const result = await runCompose(target, ["up"]);

        expect(result.exitCode).toBe(2);
        expect(result.stderr).toContain(
          `storage environment file has ${kind === "missing" ? "unavailable" : "non-directory"} ${storageEnvironmentKeys[role]}`,
        );
        expect(await Bun.file(target.dockerCalls).exists()).toBeFalse();
        expect(await Bun.file(target.composeArguments).exists()).toBeFalse();
        expect(await originalEvidence(layout.sources.library)).toEqual(before);
      } finally {
        await removeFixture(target);
      }
    });
  }
}

test("the Compose entry point forwards canonical sources to matching targets", async () => {
  const target = await fixture();
  try {
    const layout = await topology(target);
    const stateWithSpaces = join(target.root, "state with spaces");
    await mkdir(stateWithSpaces);
    layout.sources.state = stateWithSpaces;
    await writeFile(
      join(layout.sources.library, "preserved.ARW"),
      originalBytes,
    );
    await writeEnvironment(target, layout.sources);
    await writeMountCoordinates(
      target,
      Object.values(
        directMountCoordinates(
          layout.sources,
          tmpdir(),
          "/storage coordinate root with spaces",
        ),
      ),
    );
    const environmentAlias = join(target.root, "environment-alias");
    await symlink(target.environmentFile, environmentAlias);

    const result = await runCompose(target, ["up", "-d"], {
      environmentFile: environmentAlias,
      environment: {
        SLIPSTREAM_LIBRARY_ROOT: "/ambient/originals",
        SLIPSTREAM_STATE_DIRECTORY: "/ambient/state",
        SLIPSTREAM_CACHE_DIRECTORY: "/ambient/cache",
      },
    });

    expect(result.exitCode).toBe(0);
    expect(await Bun.file(target.composeArguments).text()).toBe(
      [
        "--project-name",
        "slipstream",
        "--env-file",
        await realpath(target.environmentFile),
        "-f",
        composeFile,
        "up",
        "-d",
        "",
      ].join("\n"),
    );
    expect((await Bun.file(target.composeSources).text()).split("\n")).toEqual([
      await realpath(layout.sources.library),
      await realpath(layout.sources.state),
      await realpath(layout.sources.cache),
      "",
    ]);
  } finally {
    await removeFixture(target);
  }
});

test("storage paths may contain the names of other storage keys", async () => {
  const target = await fixture();
  try {
    const layout = await topology(target);
    const stateWithKeyName = join(
      target.root,
      "state-SLIPSTREAM_LIBRARY_ROOT-backup",
    );
    await mkdir(stateWithKeyName);
    layout.sources.state = stateWithKeyName;
    await writeEnvironment(target, layout.sources);
    await writeMountCoordinates(
      target,
      Object.values(directMountCoordinates(layout.sources)),
    );

    const result = await runCompose(target, ["up"]);

    expect(result.exitCode).toBe(0);
    expect(await Bun.file(target.composeArguments).exists()).toBeTrue();
    expect((await Bun.file(target.composeSources).text()).split("\n")).toEqual([
      await realpath(layout.sources.library),
      await realpath(layout.sources.state),
      await realpath(layout.sources.cache),
      "",
    ]);
  } finally {
    await removeFixture(target);
  }
});

test("a raw non-UTF-8 storage input fails before Docker and preserves Originals", async () => {
  const target = await fixture();
  try {
    const layout = await topology(target);
    await writeFile(
      join(layout.sources.library, "preserved.ARW"),
      originalBytes,
    );
    const before = await originalSnapshot(layout.sources.library);
    const rawState = Buffer.concat([
      Buffer.from(join(target.root, "state-")),
      Buffer.from([0xff]),
      Buffer.from("-raw"),
    ]);
    await mkdir(rawState);
    await writeFile(
      target.environmentFile,
      Buffer.concat([
        Buffer.from(`SLIPSTREAM_LIBRARY_ROOT=${layout.sources.library}\n`),
        Buffer.from("SLIPSTREAM_STATE_DIRECTORY="),
        rawState,
        Buffer.from(`\nSLIPSTREAM_CACHE_DIRECTORY=${layout.sources.cache}`),
      ]),
    );

    const result = await runCompose(target, ["up"]);

    expect(result.exitCode).toBe(2);
    expect(result.stderr).toContain(
      "storage environment file has invalid SLIPSTREAM_STATE_DIRECTORY",
    );
    expect(await Bun.file(target.composeArguments).exists()).toBeFalse();
    expect(await Bun.file(target.dockerCalls).exists()).toBeFalse();
    expect(await originalSnapshot(layout.sources.library)).toEqual(before);
  } finally {
    await removeFixture(target);
  }
});

test("a canonical non-UTF-8 storage path fails before Docker and preserves Originals", async () => {
  const target = await fixture();
  try {
    const layout = await topology(target);
    await writeFile(
      join(layout.sources.library, "preserved.ARW"),
      originalBytes,
    );
    const before = await originalSnapshot(layout.sources.library);
    const rawState = Buffer.concat([
      Buffer.from(join(target.root, "state-")),
      Buffer.from([0xff]),
      Buffer.from("-target"),
    ]);
    const stateAlias = join(target.root, "state-alias");
    await mkdir(rawState);
    await symlink(rawState, stateAlias);
    await writeEnvironment(target, { ...layout.sources, state: stateAlias });

    const result = await runCompose(target, ["up"]);

    expect(result.exitCode).toBe(2);
    expect(result.stderr).toContain(
      "storage environment file has invalid SLIPSTREAM_STATE_DIRECTORY",
    );
    expect(await Bun.file(target.composeArguments).exists()).toBeFalse();
    expect(await Bun.file(target.dockerCalls).exists()).toBeFalse();
    expect(await originalSnapshot(layout.sources.library)).toEqual(before);
  } finally {
    await removeFixture(target);
  }
});

test("a raw non-UTF-8 findmnt path fails before Docker and preserves Originals", async () => {
  const target = await fixture();
  try {
    const layout = await topology(target);
    await writeEnvironment(target, layout.sources);
    await writeFile(
      join(layout.sources.library, "preserved.ARW"),
      originalBytes,
    );
    const before = await originalSnapshot(layout.sources.library);

    const result = await runCompose(target, ["up"], {
      environment: { FAKE_FINDMNT_RAW_OVERRIDE: "byte" },
    });

    expect(result.exitCode).toBe(2);
    expect(result.stderr).toContain("findmnt");
    expect(await Bun.file(target.composeArguments).exists()).toBeFalse();
    expect(await Bun.file(target.dockerCalls).exists()).toBeFalse();
    expect(await originalSnapshot(layout.sources.library)).toEqual(before);
  } finally {
    await removeFixture(target);
  }
});

for (const { label, command } of [
  { label: "startup", command: ["up"] },
  {
    label: "Library Expansion",
    command: ["run", "--rm", "--no-deps", "slipstream", "expand-library"],
  },
] as const) {
  for (const role of ["library", "state", "cache"] as const) {
    test(`a NUL inside the ${role} value fails closed before ${label}`, async () => {
      const target = await fixture();
      try {
        const layout = await topology(target);
        await expectNulEnvironmentRejected(
          target,
          layout.sources,
          nulInValueEnvironmentBytes(layout.sources, role),
          command,
        );
      } finally {
        await removeFixture(target);
      }
    });
  }
}

test("a NUL inside a storage key fails closed before Docker", async () => {
  const target = await fixture();
  try {
    const layout = await topology(target);
    await expectNulEnvironmentRejected(
      target,
      layout.sources,
      nulInKeyEnvironmentBytes(layout.sources),
      ["up"],
    );
  } finally {
    await removeFixture(target);
  }
});

test("a trailing NUL fails closed before Docker", async () => {
  const target = await fixture();
  try {
    const layout = await topology(target);
    await expectNulEnvironmentRejected(
      target,
      layout.sources,
      Buffer.concat([validEnvironmentBytes(layout.sources), Buffer.from([0])]),
      ["up"],
    );
  } finally {
    await removeFixture(target);
  }
});

test("a nested cross-filesystem bind alias fails before Docker and preserves Originals", async () => {
  const target = await fixture();
  try {
    const layout = await topology(target);
    const nestedMount = join(layout.sources.library, "nested-filesystem");
    await mkdir(nestedMount);
    await writeEnvironment(target, layout.sources);
    await writeFile(
      join(layout.sources.library, "preserved.ARW"),
      originalBytes,
    );
    const before = await originalSnapshot(layout.sources.library);
    await writeMountCoordinates(target, [
      {
        endpoint: layout.sources.library,
        target: layout.sources.library,
        fsroot: "/",
        device: "7:1",
      },
      {
        endpoint: layout.sources.library,
        target: nestedMount,
        fsroot: "/mounted-library",
        device: "8:2",
      },
      {
        endpoint: layout.sources.state,
        target: layout.sources.state,
        fsroot: "/mounted-library/state",
        device: "8:2",
      },
      {
        endpoint: layout.sources.cache,
        target: layout.sources.cache,
        fsroot: "/",
        device: "9:3",
      },
    ]);

    const result = await runCompose(target, ["up"]);

    expect(result.exitCode).toBe(2);
    expect(result.stderr).toContain(
      "host storage paths overlap: library and state",
    );
    expect(await Bun.file(target.composeArguments).exists()).toBeFalse();
    expect(await Bun.file(target.dockerCalls).exists()).toBeFalse();
    expect(await originalSnapshot(layout.sources.library)).toEqual(before);
  } finally {
    await removeFixture(target);
  }
});

for (const [left, right] of [
  ["library", "state"],
  ["library", "cache"],
  ["state", "cache"],
] as const) {
  for (const kind of ["exact", "descendant", "reverse"] as const) {
    test(`${kind} Linux bind-mount alias for ${left}-${right} never starts Compose or changes Originals tree or content`, async () => {
      const target = await fixture();
      try {
        const layout = await topology(target);
        const coordinates = directMountCoordinates(layout.sources);
        if (kind === "exact") {
          coordinates[right] = {
            endpoint: layout.sources[right],
            target: layout.sources[right],
            fsroot: layout.sources[left],
            device: "7:42",
          };
        } else if (kind === "descendant") {
          const boundSource = join(layout.sources[left], `${right}-bound`);
          await mkdir(boundSource);
          coordinates[right] = {
            endpoint: layout.sources[right],
            target: layout.sources[right],
            fsroot: boundSource,
            device: "7:42",
          };
        } else {
          const boundSource = join(layout.sources[right], `${left}-bound`);
          await mkdir(boundSource);
          coordinates[left] = {
            endpoint: layout.sources[left],
            target: layout.sources[left],
            fsroot: boundSource,
            device: "7:42",
          };
        }
        await writeMountCoordinates(target, Object.values(coordinates));
        await writeEnvironment(target, layout.sources);
        await writeFile(
          join(layout.sources.library, "preserved.ARW"),
          originalBytes,
        );
        const before = await originalSnapshot(layout.sources.library);

        const result = await runCompose(target, ["up"]);

        expect(result.exitCode).toBe(2);
        expect(result.stderr).toContain(
          `host storage paths overlap: ${left} and ${right}`,
        );
        expect(await Bun.file(target.composeArguments).exists()).toBeFalse();
        expect(await originalSnapshot(layout.sources.library)).toEqual(before);
      } finally {
        await removeFixture(target);
      }
    });
  }
}

test("an unavailable Linux mount coordinate fails closed before Compose", async () => {
  const target = await fixture();
  try {
    const layout = await topology(target);
    await writeEnvironment(target, layout.sources);
    await writeFile(
      join(layout.sources.library, "preserved.ARW"),
      originalBytes,
    );
    const before = await originalSnapshot(layout.sources.library);

    const result = await runCompose(target, ["up"], {
      environment: { FAKE_FINDMNT_FAIL: "1" },
    });

    expect(result.exitCode).toBe(2);
    expect(result.stderr).toContain("findmnt");
    expect(await Bun.file(target.composeArguments).exists()).toBeFalse();
    expect(await originalSnapshot(layout.sources.library)).toEqual(before);
  } finally {
    await removeFixture(target);
  }
});

test("invalid Linux mount coordinate fields fail closed before Compose", async () => {
  const target = await fixture();
  try {
    const layout = await topology(target);
    await writeEnvironment(target, layout.sources);
    await writeFile(
      join(layout.sources.library, "preserved.ARW"),
      originalBytes,
    );
    const before = await originalSnapshot(layout.sources.library);

    for (const [, coordinate] of [
      ["non-ancestor target", { target: "/unrelated" }],
      ["relative filesystem root", { fsroot: "relative" }],
      ["ambiguous filesystem root", { fsroot: "/unsafe#$root" }],
      ["non-numeric filesystem identity", { device: "seven:forty-two" }],
    ] as const) {
      const coordinates = directMountCoordinates(layout.sources);
      coordinates.library = { ...coordinates.library, ...coordinate };
      await writeMountCoordinates(target, Object.values(coordinates));

      const result = await runCompose(target, ["up"]);

      expect(result.exitCode).toBe(2);
      expect(result.stderr).toContain("findmnt");
      expect(await Bun.file(target.composeArguments).exists()).toBeFalse();
      expect(await originalSnapshot(layout.sources.library)).toEqual(before);
    }
  } finally {
    await removeFixture(target);
  }
});

test("ambiguous Linux mount coordinate output fails closed before Compose", async () => {
  const target = await fixture();
  try {
    const layout = await topology(target);
    await writeEnvironment(target, layout.sources);
    await writeMountCoordinates(
      target,
      Object.values(directMountCoordinates(layout.sources)),
    );
    await writeFile(
      join(layout.sources.library, "preserved.ARW"),
      originalBytes,
    );
    const before = await originalSnapshot(layout.sources.library);

    for (const environment of [
      { FAKE_FINDMNT_TRAILING_EMPTY: "1" },
      { FAKE_FINDMNT_RAW_OVERRIDE: "control" },
      { FAKE_FINDMNT_RAW_OVERRIDE: "malformed" },
    ]) {
      const result = await runCompose(target, ["up"], { environment });

      expect(result.exitCode).toBe(2);
      expect(result.stderr).toContain("findmnt");
      expect(await Bun.file(target.composeArguments).exists()).toBeFalse();
      expect(await originalSnapshot(layout.sources.library)).toEqual(before);
    }
  } finally {
    await removeFixture(target);
  }
});

for (const [left, right] of [
  ["library", "state"],
  ["library", "cache"],
  ["state", "cache"],
] as const) {
  for (const kind of ["equal", "forward", "reverse", "alias"] as const) {
    test(`${kind} ${left}-${right} topology never starts Compose or changes Originals tree or content`, async () => {
      const target = await fixture();
      try {
        const layout = await prepareInvalidTopology(target, left, right, kind);
        await writeEnvironment(target, layout.sources);
        const before = await originalSnapshot(layout.sources.library);

        const result = await runCompose(target, ["up", "-d"]);

        expect(result.exitCode).toBe(2);
        expect(result.stderr).toContain(
          `host storage paths overlap: ${left} and ${right}`,
        );
        expect(await Bun.file(target.composeArguments).exists()).toBeFalse();
        expect(await originalSnapshot(layout.sources.library)).toEqual(before);
      } finally {
        await removeFixture(target);
      }
    });
  }
}

test("Library Expansion cannot bypass a failed storage preflight", async () => {
  const target = await fixture();
  try {
    const layout = await prepareInvalidTopology(
      target,
      "library",
      "state",
      "equal",
    );
    await writeEnvironment(target, layout.sources);
    const before = await originalSnapshot(layout.sources.library);

    const result = await runCompose(target, [
      "run",
      "--rm",
      "--no-deps",
      "slipstream",
      "expand-library",
    ]);

    expect(result.exitCode).toBe(2);
    expect(result.stderr).toContain(
      "host storage paths overlap: library and state",
    );
    expect(await Bun.file(target.composeArguments).exists()).toBeFalse();
    expect(await originalSnapshot(layout.sources.library)).toEqual(before);
  } finally {
    await removeFixture(target);
  }
});

test("the storage parser rejects ambiguous declarations before Compose", async () => {
  const target = await fixture();
  try {
    const layout = await topology(target);
    const hashDirectory = `${layout.sources.cache}#alias`;
    const backslashDirectory = `${layout.sources.cache}\\alias`;
    await Promise.all([mkdir(hashDirectory), mkdir(backslashDirectory)]);
    for (const source of [
      [
        `SLIPSTREAM_LIBRARY_ROOT=${layout.sources.library}`,
        `SLIPSTREAM_LIBRARY_ROOT=${layout.sources.library}`,
        `SLIPSTREAM_STATE_DIRECTORY=${layout.sources.state}`,
        `SLIPSTREAM_CACHE_DIRECTORY=${layout.sources.cache}`,
      ],
      [
        `SLIPSTREAM_LIBRARY_ROOT="${layout.sources.library}"`,
        `SLIPSTREAM_STATE_DIRECTORY=${layout.sources.state}`,
        `SLIPSTREAM_CACHE_DIRECTORY=${layout.sources.cache}`,
      ],
      [
        `SLIPSTREAM_LIBRARY_ROOT=${layout.sources.library}`,
        `SLIPSTREAM_STATE_DIRECTORY=${layout.sources.state}`,
        "SLIPSTREAM_CACHE_DIRECTORY=$" + "{SLIPSTREAM_CACHE}",
      ],
      [
        `SLIPSTREAM_LIBRARY_ROOT=${layout.sources.library}`,
        `SLIPSTREAM_STATE_DIRECTORY=${layout.sources.state}`,
        `SLIPSTREAM_CACHE_DIRECTORY=${hashDirectory}`,
      ],
      [
        `SLIPSTREAM_LIBRARY_ROOT=${layout.sources.library}`,
        `SLIPSTREAM_STATE_DIRECTORY=${layout.sources.state}`,
        `SLIPSTREAM_CACHE_DIRECTORY=${backslashDirectory}`,
      ],
    ]) {
      await writeFile(target.environmentFile, source.join("\n"));

      const result = await runCompose(target, ["up"]);

      expect(result.exitCode).toBe(2);
      expect(result.stderr).toContain("storage environment file");
      expect(await Bun.file(target.composeArguments).exists()).toBeFalse();
    }
  } finally {
    await removeFixture(target);
  }
});

test("a safe alias cannot hide an ambiguous canonical storage path", async () => {
  const target = await fixture();
  try {
    const layout = await topology(target);
    await writeFile(
      join(layout.sources.library, "preserved.ARW"),
      originalBytes,
    );
    const before = await originalSnapshot(layout.sources.library);

    for (const [index, suffix] of [
      "hash#target",
      "dollar$target",
      "newline\ntarget",
      "tab\ttarget",
      "trailing-newline\n",
    ].entries()) {
      const ambiguousTarget = join(target.root, `ambiguous-${suffix}`);
      const safeAlias = join(target.root, `state-alias-${index}`);
      await mkdir(ambiguousTarget);
      if (suffix.endsWith("\n")) {
        await mkdir(ambiguousTarget.slice(0, -1));
      }
      await symlink(ambiguousTarget, safeAlias);
      await writeEnvironment(target, { ...layout.sources, state: safeAlias });

      const result = await runCompose(target, ["up"]);

      expect(result.exitCode).toBe(2);
      expect(result.stderr).toContain(
        "storage environment file has invalid SLIPSTREAM_STATE_DIRECTORY",
      );
      expect(await Bun.file(target.composeArguments).exists()).toBeFalse();
      expect(await originalSnapshot(layout.sources.library)).toEqual(before);
    }
  } finally {
    await removeFixture(target);
  }
});

test("unsupported Compose overrides cannot follow the preflight", async () => {
  const target = await fixture();
  try {
    const layout = await topology(target);
    await writeEnvironment(target, layout.sources);
    for (const command of [
      ["start"],
      ["up -d"],
      ["up", "--detach"],
      ["up", "--env-file", "other.env"],
      ["up", "-f", "other-compose.yaml"],
      [
        "run",
        "--rm",
        "--no-deps",
        "-v",
        "/tmp:/tmp",
        "slipstream",
        "expand-library",
      ],
      [
        "run",
        "--rm",
        "--no-deps",
        "--volume",
        "/tmp:/tmp",
        "slipstream",
        "expand-library",
      ],
      [
        "run",
        "--rm",
        "--no-deps",
        "-e",
        "SLIPSTREAM_STATE_DIRECTORY=/tmp",
        "slipstream",
        "expand-library",
      ],
      [
        "run",
        "--rm",
        "--no-deps",
        "--env",
        "SLIPSTREAM_CACHE_DIRECTORY=/tmp",
        "slipstream",
        "expand-library",
      ],
      [
        "run",
        "--rm",
        "--no-deps",
        "slipstream",
        "--entrypoint",
        "/bin/sh",
        "expand-library",
      ],
    ]) {
      const result = await runCompose(target, command);

      expect(result.exitCode).toBe(2);
      expect(result.stderr).toContain("unsupported Compose command");
      expect(await Bun.file(target.composeArguments).exists()).toBeFalse();
    }
  } finally {
    await removeFixture(target);
  }
});

test("remote Docker selection is rejected before Compose", async () => {
  const target = await fixture();
  try {
    const layout = await topology(target);
    await writeEnvironment(target, layout.sources);
    for (const environment of [
      { DOCKER_HOST: "tcp://remote.example:2375" },
      { DOCKER_CONTEXT: "remote" },
      { DOCKER_HOST: "" },
      { DOCKER_CONTEXT: "" },
      { FAKE_DOCKER_CONTEXT_ENDPOINT: "tcp://remote.example:2375" },
      { FAKE_DOCKER_CONTEXT_ENDPOINT: "unix://" },
      { FAKE_DOCKER_CONTEXT_ENDPOINT: "unix:///tmp/docker\n" },
    ]) {
      const result = await runCompose(target, ["up"], { environment });

      expect(result.exitCode).toBe(2);
      expect(result.stderr).toContain("local Docker Engine");
      expect(await Bun.file(target.composeArguments).exists()).toBeFalse();
    }
  } finally {
    await removeFixture(target);
  }
});

test("the entry point clears Compose configuration-selection process variables", async () => {
  const target = await fixture();
  try {
    const layout = await topology(target);
    await writeEnvironment(target, layout.sources);

    const result = await runCompose(target, ["up"], {
      environment: {
        COMPOSE_ENV_FILES: "other.env",
        COMPOSE_FILE: "other-compose.yaml",
        COMPOSE_PROFILES: "other",
        COMPOSE_PROJECT_NAME: "other",
      },
    });

    expect(result.exitCode).toBe(0);
    expect(await Bun.file(target.composeEnvironment).text()).toBe("\n\n\n\n");
  } finally {
    await removeFixture(target);
  }
});

test("environment-file configuration wins over ambient values", async () => {
  const target = await fixture();
  try {
    const layout = await topology(target);
    await writeEnvironment(target, layout.sources);

    const result = await runCompose(target, ["up"], {
      environment: {
        SLIPSTREAM_IMAGE: "slipstream:ambient",
        SLIPSTREAM_VCS_REF: "ambient-ref",
        SLIPSTREAM_BIND_ADDRESS: "0.0.0.0",
        SLIPSTREAM_PORT: "3999",
        SLIPSTREAM_PUBLIC_ORIGIN: "https://ambient.invalid",
        SLIPSTREAM_DATABASE_BASENAME: "ambient.sqlite",
      },
    });

    expect(result.exitCode).toBe(0);
    expect(await Bun.file(target.composeArguments).text()).toContain(
      `--env-file\n${await realpath(target.environmentFile)}\n`,
    );
    expect(await Bun.file(target.composeConfiguration).text()).toBe(
      "\n\n\n\n\n\n",
    );
  } finally {
    await removeFixture(target);
  }
});

test("the environment file cannot select a Compose project, profile, file, or Docker context", async () => {
  const target = await fixture();
  try {
    const layout = await topology(target);
    for (const declaration of [
      "COMPOSE_PROJECT_NAME=other",
      "COMPOSE_PROJECT_NAME = other",
      "export COMPOSE_PROFILES=other",
      "COMPOSE_FILE=other-compose.yaml",
      "COMPOSE_ENV_FILES=other.env",
      "DOCKER_CONTEXT=remote",
      "DOCKER_HOST=tcp://remote.example:2375",
      "COMPOSE_1=other",
    ]) {
      await writeFile(
        target.environmentFile,
        [
          `SLIPSTREAM_LIBRARY_ROOT=${layout.sources.library}`,
          `SLIPSTREAM_STATE_DIRECTORY=${layout.sources.state}`,
          `SLIPSTREAM_CACHE_DIRECTORY=${layout.sources.cache}`,
          declaration,
        ].join("\n"),
      );

      for (const command of [["up"], ["down"]]) {
        const result = await runCompose(target, command);

        expect(result.exitCode).toBe(2);
        expect(result.stderr).toContain(
          "must not declare Compose or Docker control variables",
        );
        expect(await Bun.file(target.composeArguments).exists()).toBeFalse();
      }
    }
  } finally {
    await removeFixture(target);
  }
});

test("down uses the fixed local Compose route without storage preflight", async () => {
  const target = await fixture();
  try {
    const layout = await topology(target);
    layout.sources.library = join(target.root, "missing Originals");
    await writeEnvironment(target, layout.sources);

    const result = await runCompose(target, ["down"], {
      environment: {
        SLIPSTREAM_LIBRARY_ROOT: "/ambient/originals",
        SLIPSTREAM_STATE_DIRECTORY: "/ambient/state",
        SLIPSTREAM_CACHE_DIRECTORY: "/ambient/cache",
      },
    });

    expect(result.exitCode).toBe(0);
    expect(await Bun.file(target.composeArguments).text()).toBe(
      [
        "--project-name",
        "slipstream",
        "--env-file",
        await realpath(target.environmentFile),
        "-f",
        composeFile,
        "down",
        "",
      ].join("\n"),
    );
    expect(await Bun.file(target.composeSources).text()).toBe("\n\n\n");
  } finally {
    await removeFixture(target);
  }
});

test("Docker Compose config preserves the storage and environment-file contract", async () => {
  const target = await fixture();
  try {
    const layout = await topology(target);
    await writeEnvironment(target, layout.sources);

    const configuration = await dockerComposeConfig(target.environmentFile);
    const services = configuration.services as Record<string, unknown>;
    expect(services).toBeDefined();
    const service = services.slipstream as Record<string, unknown>;
    expect(service).toBeDefined();
    const build = service.build as Record<string, unknown>;
    const buildArguments = build.args as Record<string, unknown>;
    const environment = service.environment as Record<string, unknown>;
    const ports = service.ports as Array<Record<string, unknown>>;
    const volumes = service.volumes as Array<Record<string, unknown>>;

    expect(service.read_only).toBe(true);
    expect(service.user).toBe("1000:1000");
    expect(service.cap_drop).toEqual(["ALL"]);
    expect(service.security_opt).toEqual(["no-new-privileges:true"]);
    expect(service.image).toBe("slipstream:from-environment-file");
    expect(buildArguments.SLIPSTREAM_VCS_REF).toBe("environment-file-ref");
    expect(environment).toMatchObject({
      SLIPSTREAM_DATABASE_BASENAME: "environment-file.sqlite",
      SLIPSTREAM_PUBLIC_ORIGIN: "https://environment-file.invalid",
    });
    expect(ports).toContainEqual(
      expect.objectContaining({
        host_ip: "127.0.0.2",
        published: "3100",
        target: 3000,
      }),
    );

    const bindMounts = volumes
      .filter((candidate) => candidate.type === "bind")
      .map(
        (candidate) =>
          `${String(candidate.source)}\0${String(candidate.target)}`,
      )
      .sort();
    expect(bindMounts).toEqual(
      Object.values(layout.sources)
        .map((source) => `${source}\0${source}`)
        .sort(),
    );

    for (const role of ["library", "state", "cache"] as const) {
      const source = layout.sources[role];
      const key = storageEnvironmentKeys[role];
      const volume = volumes.find(
        (candidate) =>
          candidate.type === "bind" &&
          candidate.source === source &&
          candidate.target === source,
      );

      expect(environment[key]).toBe(source);
      expect(volume).toBeDefined();
      expect(volume?.source).toBe(environment[key]);
      expect(volume?.target).toBe(environment[key]);
      expect(volume?.read_only ?? false).toBe(role === "library");
    }

    expect(service.restart).toBeUndefined();
    expect(service.restart_policy).toBeUndefined();
  } finally {
    await removeFixture(target);
  }
});

test("Compose mounts and server configuration use the same storage variables", async () => {
  const source = await Bun.file(composeFile).text();
  for (const variable of [
    "SLIPSTREAM_LIBRARY_ROOT",
    "SLIPSTREAM_STATE_DIRECTORY",
    "SLIPSTREAM_CACHE_DIRECTORY",
  ]) {
    const reference = `\${${variable}:?`;
    expect(source).toContain(`      ${variable}: ${reference}`);
    expect(source).toContain(`        source: ${reference}`);
    expect(source).toContain(`        target: ${reference}`);
  }
});

test("Compose has no autonomous restart policy", async () => {
  const source = await Bun.file(composeFile).text();
  expect(source).not.toMatch(/^\s+restart(?:_policy)?\s*:/m);
});
