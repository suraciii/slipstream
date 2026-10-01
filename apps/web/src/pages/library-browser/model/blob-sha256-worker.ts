import { sha256 } from "@noble/hashes/sha256";

const worker = self as unknown as {
  onmessage: (event: MessageEvent<Blob>) => void;
  postMessage: (digest: string | null) => void;
};

async function hashBlob(blob: Blob): Promise<void> {
  const hash = sha256.create();
  try {
    const chunkBytes = 1024 * 1024;
    for (let offset = 0; offset < blob.size; offset += chunkBytes) {
      const bytes = await blob.slice(offset, offset + chunkBytes).arrayBuffer();
      hash.update(new Uint8Array(bytes));
    }
    worker.postMessage(
      Array.from(hash.digest(), (byte) =>
        byte.toString(16).padStart(2, "0"),
      ).join(""),
    );
  } catch {
    worker.postMessage(null);
  } finally {
    hash.destroy();
  }
}

worker.onmessage = ({ data }) => {
  void hashBlob(data);
};
