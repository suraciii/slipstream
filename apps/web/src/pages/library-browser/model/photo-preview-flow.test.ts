import { describe, expect, test } from "bun:test";
import { createPreviewFailureRefresh } from "./photo-preview-flow.js";
import type {
  PhotoAuthority,
  PhotoOwnerEvent,
} from "./photo-preview-outcome.js";

const event = (url: string): PhotoOwnerEvent => ({
  kind: "review-image-failed",
  authority: Object.freeze({}) as PhotoAuthority,
  photoId: "photo-1",
  url,
  surface: {},
  photoSurface: {},
});

describe("Preview failure refresh", () => {
  test("claims one refresh per failed image URL", () => {
    const refreshed: string[] = [];
    const gate = createPreviewFailureRefresh((failure) =>
      refreshed.push(failure.url),
    );

    expect(gate.claim(event("/review-a.jpg"))).toBe(true);
    expect(gate.claim(event("/review-a.jpg"))).toBe(false);
    expect(gate.claim(event("/review-b.jpg"))).toBe(true);
    expect(refreshed).toEqual(["/review-a.jpg", "/review-b.jpg"]);
  });

  test("evicts the oldest URL so a later failure can retry again", () => {
    const refreshed: string[] = [];
    const gate = createPreviewFailureRefresh(
      (failure) => refreshed.push(failure.url),
      2,
    );

    expect(gate.claim(event("/review-a.jpg"))).toBe(true);
    expect(gate.claim(event("/review-b.jpg"))).toBe(true);
    expect(gate.claim(event("/review-c.jpg"))).toBe(true);
    expect(gate.claim(event("/review-a.jpg"))).toBe(true);
    expect(gate.claim(event("/review-b.jpg"))).toBe(true);
    expect(refreshed).toEqual([
      "/review-a.jpg",
      "/review-b.jpg",
      "/review-c.jpg",
      "/review-a.jpg",
      "/review-b.jpg",
    ]);
  });
});
