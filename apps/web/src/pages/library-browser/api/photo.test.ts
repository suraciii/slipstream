import { describe, expect, test } from "bun:test";
import { fetchPhotoAlbums, fetchPhotoMetadata } from "./photo.js";

const photoId = "photo-1";
const signal = new AbortController().signal;

const okJson = (body: unknown, status = 200): Response =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });

describe("Photo Albums API", () => {
  test("accepts an Album identity listing", async () => {
    const result = await fetchPhotoAlbums(
      () =>
        Promise.resolve(
          okJson({ albums: [{ id: "album-1", name: "Keepers" }] }),
        ),
      photoId,
      signal,
    );

    expect(result).toEqual({
      kind: "ok",
      value: { albums: [{ id: "album-1", name: "Keepers" }] },
    });
  });

  test("keeps an answered Album refusal failed with its status", async () => {
    const result = await fetchPhotoAlbums(
      () => Promise.resolve(okJson({ error: "gone" }, 503)),
      photoId,
      signal,
    );

    expect(result).toEqual({ kind: "failed", status: 503 });
  });

  test("keeps a lost transport failed with no status", async () => {
    const result = await fetchPhotoAlbums(
      () => Promise.reject(new Error("offline")),
      photoId,
      signal,
    );

    expect(result).toEqual({ kind: "failed" });
  });

  test("reports a non-JSON answer as failed, never a throw", async () => {
    const result = await fetchPhotoAlbums(
      () =>
        Promise.resolve(
          new Response("<html>proxy error</html>", { status: 200 }),
        ),
      photoId,
      signal,
    );

    expect(result).toEqual({ kind: "failed" });
  });

  test("rejects a listing that is not an Album collection", async () => {
    const result = await fetchPhotoAlbums(
      () => Promise.resolve(okJson({ albums: [{ id: "", name: "x" }] })),
      photoId,
      signal,
    );

    expect(result).toEqual({ kind: "failed" });
  });
});

describe("Photo metadata API", () => {
  test("accepts a metadata answer", async () => {
    const result = await fetchPhotoMetadata(
      () =>
        Promise.resolve(
          okJson({
            captureTime: "2024-01-02",
            aperture: "f/1.8",
            iso: 100,
            shutterSpeed: "1/250",
            focalLength: "35mm",
          }),
        ),
      photoId,
      signal,
    );

    expect(result).toEqual({
      kind: "ok",
      value: {
        captureTime: "2024-01-02",
        aperture: "f/1.8",
        iso: 100,
        shutterSpeed: "1/250",
        focalLength: "35mm",
      },
    });
  });

  test("accepts an empty metadata answer", async () => {
    const result = await fetchPhotoMetadata(
      () => Promise.resolve(okJson({})),
      photoId,
      signal,
    );

    expect(result).toEqual({ kind: "ok", value: {} });
  });

  test("keeps an answered metadata refusal failed with its status", async () => {
    const result = await fetchPhotoMetadata(
      () => Promise.resolve(okJson({ error: "gone" }, 503)),
      photoId,
      signal,
    );

    expect(result).toEqual({ kind: "failed", status: 503 });
  });

  test("keeps a lost transport failed with no status", async () => {
    const result = await fetchPhotoMetadata(
      () => Promise.reject(new Error("offline")),
      photoId,
      signal,
    );

    expect(result).toEqual({ kind: "failed" });
  });

  test("reports a non-JSON answer as failed, never a throw", async () => {
    const result = await fetchPhotoMetadata(
      () =>
        Promise.resolve(
          new Response("<html>proxy error</html>", { status: 200 }),
        ),
      photoId,
      signal,
    );

    expect(result).toEqual({ kind: "failed" });
  });

  test("rejects a metadata answer with a wrong field type", async () => {
    const result = await fetchPhotoMetadata(
      () => Promise.resolve(okJson({ iso: "100" })),
      photoId,
      signal,
    );

    expect(result).toEqual({ kind: "failed" });
  });
});
