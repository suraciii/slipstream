import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { expect, test } from "bun:test";

const script = new URL("../scripts/prepare-ci-libraw.sh", import.meta.url)
  .pathname;

async function run(command: string[], cwd: string, env = process.env) {
  const child = Bun.spawn(command, {
    cwd,
    env,
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  return { output: stdout + stderr, exitCode };
}

async function fixture() {
  const root = await mkdtemp(join(tmpdir(), "slipstream-ci-libraw-"));
  const prefix = join(root, "libraw-022");
  const environmentFile = join(root, "environment");
  const download = join(root, "download-attempted");
  await mkdir(join(prefix, "lib", "pkgconfig"), { recursive: true });
  await mkdir(join(prefix, "include", "libraw"), { recursive: true });
  await mkdir(join(root, "bin"));
  await writeFile(
    join(root, "bin", "curl"),
    `#!/usr/bin/env bash\ntouch "${download}"\nexit 79\n`,
  );
  await chmod(join(root, "bin", "curl"), 0o755);
  await writeFile(
    join(prefix, "include", "libraw", "libraw.h"),
    "fixture header\n",
  );
  for (const name of ["libraw", "libraw_r"]) {
    await writeFile(
      join(prefix, "lib", `${name}.so.25.0.0`),
      "fixture shared library\n",
    );
    await symlink(`${name}.so.25.0.0`, join(prefix, "lib", `${name}.so`));
    await symlink(`${name}.so.25.0.0`, join(prefix, "lib", `${name}.so.25`));
    await writeFile(
      join(prefix, "lib", "pkgconfig", `${name}.pc`),
      [
        `prefix=${prefix}`,
        "libdir=${prefix}/lib",
        "includedir=${prefix}/include",
        `Name: ${name}`,
        "Description: isolated LibRaw prefix fixture",
        "Version: 0.22.2",
        "Libs: -L${libdir} -lraw",
        "Cflags: -I${includedir}",
        "",
      ].join("\n"),
    );
  }
  async function seal() {
    const result = await run(
      [
        "bash",
        "-c",
        "find . -type f ! -name checksums.sha256 -print0 | LC_ALL=C sort -z | xargs -0 sha256sum",
      ],
      prefix,
    );
    if (result.exitCode !== 0) throw new Error(result.output);
    await writeFile(join(prefix, "checksums.sha256"), result.output);
  }
  await seal();
  return {
    root,
    prefix,
    environmentFile,
    download,
    seal,
    prepare: () =>
      run(["bash", script], root, {
        ...process.env,
        PATH: `${join(root, "bin")}:${process.env.PATH ?? ""}`,
        RUNNER_TEMP: root,
        GITHUB_ENV: environmentFile,
        PKG_CONFIG_PATH: "",
        LD_LIBRARY_PATH: "",
      }),
  };
}

test("a complete restored prefix is checked and selected without downloading", async () => {
  const f = await fixture();
  try {
    const result = await f.prepare();
    expect(result.exitCode).toBe(0);
    expect(await readFile(f.environmentFile, "utf8")).toBe(
      `PKG_CONFIG_PATH=${f.prefix}/lib/pkgconfig\nLD_LIBRARY_PATH=${f.prefix}/lib\n`,
    );
    expect(await Bun.file(f.download).exists()).toBeFalse();
  } finally {
    await rm(f.root, { recursive: true, force: true });
  }
});

for (const fault of [
  "missing-manifest",
  "empty-manifest",
  "corrupt-library",
  "wrong-version",
  "foreign-prefix",
  "escaped-library",
  "escaped-soname",
  "missing-soname",
] as const) {
  test(`a restored ${fault} refuses without fallback or environment publication`, async () => {
    const f = await fixture();
    try {
      const manifest = join(f.prefix, "checksums.sha256");
      const pc = join(f.prefix, "lib", "pkgconfig", "libraw_r.pc");
      if (fault === "missing-manifest") await rm(manifest);
      if (fault === "empty-manifest") await writeFile(manifest, "");
      if (fault === "corrupt-library")
        await writeFile(
          join(f.prefix, "lib", "libraw.so.25.0.0"),
          "corrupted\n",
        );
      if (fault === "wrong-version" || fault === "foreign-prefix") {
        const content = await readFile(pc, "utf8");
        await writeFile(
          pc,
          fault === "wrong-version"
            ? content.replace("0.22.2", "0.21.4")
            : content.replace(`prefix=${f.prefix}`, "prefix=/usr"),
        );
        await f.seal();
      }
      if (fault === "escaped-library" || fault === "escaped-soname") {
        const outside = join(f.root, "outside.so");
        await writeFile(outside, "outside\n");
        const name =
          fault === "escaped-library" ? "libraw.so" : "libraw_r.so.25";
        await rm(join(f.prefix, "lib", name));
        await symlink(outside, join(f.prefix, "lib", name));
      }
      if (fault === "missing-soname")
        await rm(join(f.prefix, "lib", "libraw_r.so.25"));
      const result = await f.prepare();
      expect(result.exitCode).not.toBe(0);
      expect(await Bun.file(f.environmentFile).exists()).toBeFalse();
      expect(await Bun.file(f.download).exists()).toBeFalse();
    } finally {
      await rm(f.root, { recursive: true, force: true });
    }
  });
}

test("a missing prefix with a failed download publishes no environment", async () => {
  const f = await fixture();
  try {
    await rm(f.prefix, { recursive: true });
    const result = await f.prepare();
    expect(result.exitCode).toBe(79);
    expect(await Bun.file(f.environmentFile).exists()).toBeFalse();
    expect(
      await Bun.file(join(f.prefix, "checksums.sha256")).exists(),
    ).toBeFalse();
  } finally {
    await rm(f.root, { recursive: true, force: true });
  }
});
