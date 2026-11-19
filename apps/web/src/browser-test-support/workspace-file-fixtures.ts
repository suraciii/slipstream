import type { Route } from "@playwright/test";
import { createHash } from "node:crypto";
import type { WorkspaceFile } from "../pages/library-browser/index.js";

export const windowOf = <T>(items: T[]) => ({
  items,
  total: items.length,
  nextCursor: null,
});

export const json = (route: Route, body: unknown, status = 200) =>
  route.fulfill({
    status,
    contentType: "application/json",
    body: JSON.stringify(body),
  });

export function fileFixture(
  artifactId: string,
  bytes: Buffer,
  revision = "file-r1",
  kind = "output",
  relativePath = `${kind}s/${artifactId}.tif`,
): WorkspaceFile {
  return {
    artifactId,
    revision,
    relativePath,
    kind,
    state: "ready",
    byteLength: bytes.length,
    sha256: createHash("sha256").update(bytes).digest("hex"),
    imageContract: {
      format: "tiff",
      precision: "float32",
      transfer: "linear",
      colorSpace: "linear-rec2020",
      width: 16,
      height: 16,
    },
  };
}
