import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

import type { EmbeddedJpegOutcome } from "./libraw-preview.js";

type TestCandidate = Readonly<{
  declaredLength: number;
  jpeg: Buffer;
  unpackOutcome?: "success" | "data" | "resource" | "io" | "internal";
}>;
type TestSelectionOutcome = EmbeddedJpegOutcome &
  Readonly<{ extractedCandidateIndexes: number[] }>;

type NativeTestBinding = Readonly<{
  extractLargestEmbeddedJpeg(path: string): EmbeddedJpegOutcome;
  __testCreateJpeg(width: number, height: number): Buffer;
  __testSelectCandidates(candidates: TestCandidate[]): TestSelectionOutcome;
  __testReadWholeWithMutation(
    path: string,
    mutate: () => void,
  ): { kind: string; bytes?: Buffer; message?: string };
  __testExtractWithMutation(
    path: string,
    mutate: () => void,
  ): EmbeddedJpegOutcome;
}>;

const require = createRequire(import.meta.url);
const addonPath = fileURLToPath(
  new URL("../../build/Release/raw_preview_test.node", import.meta.url),
);
const binding = require(addonPath) as NativeTestBinding;

export const extractTestPathEmbeddedJpeg = binding.extractLargestEmbeddedJpeg;
export const createTestJpeg = binding.__testCreateJpeg;
export const selectTestCandidates = binding.__testSelectCandidates;
export const readWholeWithMutation = binding.__testReadWholeWithMutation;
export const extractWithMutation = binding.__testExtractWithMutation;
