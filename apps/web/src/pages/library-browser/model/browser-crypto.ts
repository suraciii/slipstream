import { sha256 } from "@noble/hashes/sha256";

/** Returns a version 4 UUID using the browser's cryptographically secure RNG. */
export function randomUuid(): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  bytes[6] = (bytes[6]! & 0x0f) | 0x40;
  bytes[8] = (bytes[8]! & 0x3f) | 0x80;
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0"));
  return `${hex.slice(0, 4).join("")}-${hex.slice(4, 6).join("")}-${hex.slice(6, 8).join("")}-${hex.slice(8, 10).join("")}-${hex.slice(10).join("")}`;
}

const hashBlobWithNoble = async (blob: Blob): Promise<string> => {
  const hash = sha256.create();
  try {
    const chunkBytes = 1024 * 1024;
    for (let offset = 0; offset < blob.size; offset += chunkBytes) {
      const bytes = await blob.slice(offset, offset + chunkBytes).arrayBuffer();
      hash.update(new Uint8Array(bytes));
    }
    return Array.from(hash.digest(), (byte) =>
      byte.toString(16).padStart(2, "0"),
    ).join("");
  } finally {
    hash.destroy();
  }
};

export async function blobSha256Hex(blob: Blob): Promise<string> {
  if (crypto.subtle) {
    const digest = await crypto.subtle.digest(
      "SHA-256",
      await blob.arrayBuffer(),
    );
    return Array.from(new Uint8Array(digest), (byte) =>
      byte.toString(16).padStart(2, "0"),
    ).join("");
  }
  const { promise, resolve, reject } = Promise.withResolvers<string>();
  let worker: Worker;
  try {
    worker = new Worker(new URL("./blob-sha256-worker.ts", import.meta.url), {
      type: "module",
    });
  } catch {
    return hashBlobWithNoble(blob);
  }
  let settled = false;
  let fallbackStarted = false;
  const fail = () => {
    if (settled || fallbackStarted) return;
    fallbackStarted = true;
    worker.terminate();
    void hashBlobWithNoble(blob).then(
      (value) => {
        settled = true;
        resolve(value);
      },
      () => {
        settled = true;
        reject(new Error("Could not verify the downloaded file."));
      },
    );
  };
  worker.onmessage = (event: MessageEvent<string | null>) => {
    if (settled || fallbackStarted) return;
    if (typeof event.data === "string") {
      settled = true;
      worker.terminate();
      resolve(event.data);
    } else fail();
  };
  worker.onerror = fail;
  worker.onmessageerror = fail;
  try {
    worker.postMessage(blob);
  } catch {
    fail();
  }
  return promise;
}
