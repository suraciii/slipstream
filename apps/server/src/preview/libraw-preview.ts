import { accessSync, constants } from "node:fs";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

export type EmbeddedJpegOutcome =
  | Readonly<{
      kind: "preview";
      candidateIndex: number;
      width: number;
      height: number;
      jpeg: Buffer;
    }>
  | Readonly<{ kind: "unsupported"; message: string }>
  | Readonly<{ kind: "malformed"; message: string }>
  | Readonly<{ kind: "io-error"; message: string }>
  | Readonly<{ kind: "resource-limit"; message: string }>
  | Readonly<{ kind: "internal-error"; message: string }>
  | Readonly<{ kind: "no-usable-preview"; message: string }>;

type NativeBinding = Readonly<{
  extractLargestEmbeddedJpeg(path: string): EmbeddedJpegOutcome;
}>;

const require = createRequire(import.meta.url);
const addonPath = fileURLToPath(
  new URL("../../build/Release/raw_preview.node", import.meta.url),
);
let binding: NativeBinding | undefined;

function nativeBinding(): NativeBinding {
  binding ??= require(addonPath) as NativeBinding;
  return binding;
}

export function extractLargestEmbeddedJpeg(path: string): EmbeddedJpegOutcome {
  if (typeof path !== "string") {
    throw new TypeError("Expected one RAW file path");
  }
  try {
    accessSync(path, constants.R_OK);
  } catch (error) {
    return {
      kind: "io-error",
      message:
        error instanceof Error
          ? error.message
          : "Original File is not readable",
    };
  }
  return nativeBinding().extractLargestEmbeddedJpeg(path);
}
