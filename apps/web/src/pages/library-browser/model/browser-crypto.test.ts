import { describe, expect, test } from "bun:test";
import { randomUuid, blobSha256Hex } from "./browser-crypto.js";

describe("browser crypto helpers", () => {
  test("creates RFC 4122 version 4 UUIDs with secure-random identity", () => {
    const value = randomUuid();
    expect(value).toMatch(
      /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/,
    );
  });

  test("computes the complete Blob SHA-256", async () => {
    expect(await blobSha256Hex(new Blob(["abc"]))).toBe(
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
    expect(await blobSha256Hex(new Blob())).toBe(
      "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    );
  });
});
