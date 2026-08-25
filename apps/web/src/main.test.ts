import { describe, expect, it } from "vitest";

import { renderApp } from "./main.js";

describe("Web entry point", () => {
  it("renders the application identity", () => {
    const heading = { textContent: "" };
    const root = {
      replaceChildren(child: unknown) {
        expect(child).toBe(heading);
      },
    };
    const previousDocument = globalThis.document;
    globalThis.document = {
      createElement() {
        return heading;
      },
      querySelector() {
        return null;
      },
    } as unknown as Document;

    try {
      renderApp(root as unknown as HTMLElement);
      expect(heading.textContent).toBe("Slipstream");
    } finally {
      globalThis.document = previousDocument;
    }
  });
});
