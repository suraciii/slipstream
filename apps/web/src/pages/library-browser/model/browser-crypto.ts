/** Returns a version 4 UUID using the browser's cryptographically secure RNG. */
export function randomUuid(): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  bytes[6] = (bytes[6]! & 0x0f) | 0x40;
  bytes[8] = (bytes[8]! & 0x3f) | 0x80;
  const hex = Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0"));
  return `${hex.slice(0, 4).join("")}-${hex.slice(4, 6).join("")}-${hex.slice(6, 8).join("")}-${hex.slice(8, 10).join("")}-${hex.slice(10).join("")}`;
}

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
  const worker = new Worker(
    new URL("./blob-sha256-worker.ts", import.meta.url),
    {
      type: "module",
    },
  );
  const fail = () => {
    worker.terminate();
    reject(new Error("Could not verify the downloaded file."));
  };
  worker.onmessage = (event: MessageEvent<string | null>) => {
    if (typeof event.data === "string") {
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
