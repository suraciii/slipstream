import { createRequire } from "node:module";

export type OriginalKind = "raw" | "jpeg";
type FileKindsRuntime = Readonly<{
  rawExtensions: ReadonlyArray<string>;
  jpegExtensions: ReadonlyArray<string>;
  classifyOriginalFile(name: string): OriginalKind | undefined;
  pairingBaseName(name: string): string;
}>;

const runtime = createRequire(import.meta.url)(
  "./file-kinds.cjs",
) as FileKindsRuntime;
export const rawExtensions = new Set(runtime.rawExtensions);
export const jpegExtensions = new Set(runtime.jpegExtensions);
export const classifyOriginalFile = runtime.classifyOriginalFile;
export const pairingBaseName = runtime.pairingBaseName;
