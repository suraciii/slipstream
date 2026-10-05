import { describe, expect, test } from "bun:test";
import type { BrowserFetch } from "../model/access-session.js";
import {
  fetchProcessingPreview,
  processingPreviewUri,
} from "./processing-preview.js";

describe("processing preview wire contract", () => {
  test("uses the current Edit State route without selecting a predecessor", () => {
    expect(processingPreviewUri("photo/one", "film-2")).toBe(
      "/api/photos/photo%2Fone/edit/preview",
    );
  });

  test("fetches only the current Edit State and returns the service's answer untouched", async () => {
    const requests: Array<{ input: unknown; init: RequestInit | undefined }> =
      [];
    const fetcher: BrowserFetch = (input, init) => {
      requests.push({ input, init });
      return Promise.resolve(
        new Response(
          JSON.stringify({ error: { code: "module_parameters_unavailable" } }),
          { status: 503 },
        ),
      );
    };
    const controller = new AbortController();
    const response = await fetchProcessingPreview(
      fetcher,
      "photo/one",
      "film-2",
      controller.signal,
    );
    expect(response.status).toBe(503);
    // The client owns one route: a refusal is handed back as the service
    // answered it, with no predecessor step, fallback, or second request.
    expect(requests.length).toBe(1);
    expect(String(requests[0]?.input)).toBe(
      processingPreviewUri("photo/one", "film-2"),
    );
    expect(requests[0]?.init?.signal).toBe(controller.signal);
    expect(requests[0]?.init?.priority).toBe("high");
    expect(await response.json()).toEqual({
      error: { code: "module_parameters_unavailable" },
    });
  });
});
