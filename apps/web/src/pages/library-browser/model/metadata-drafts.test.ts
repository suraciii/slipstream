import { describe, expect, test } from "bun:test";
import type { MetadataRead } from "../api/external-metadata.js";
import {
  createMetadataDrafts,
  defaultLanguageChange,
  metadataInputChange,
  type MetadataInput,
} from "./metadata-drafts.js";

function facts(photoId: string): MetadataRead {
  return {
    photoId,
    originalLocation: `${photoId}.jpg`,
    association: { state: "eligible", candidates: [] },
    evidence: { sidecar: { state: "absent" } },
    fields: {
      "dc:title": {
        state: "present",
        writable: true,
        sources: [],
        value: { "x-default": "Old", fr: "Ancien" },
      },
    },
    captureFacts: {},
    libraryRating: 0,
    saveAvailable: true,
  };
}
const input = (text: string, operation = "set"): MetadataInput => ({
  text,
  operation,
  languages: [],
});

describe("metadata draft input", () => {
  test("ordinary Title and Description change only the default language", () => {
    for (const key of ["dc:title", "dc:description"]) {
      const value = input("New");
      value.languages = [{ name: "fr", value: "Ancien", action: "unchanged" }];
      expect(metadataInputChange(key, "languages", value)).toEqual({
        change: { op: "setLanguages", languages: { "x-default": "New" } },
      });
    }
  });
  test("additional named language changes preserve unmentioned alternatives", () => {
    const value = input("", "unchanged");
    value.languages = [
      { name: "fr", value: "Nouveau", action: "change" },
      { name: "de", value: "Alt", action: "unchanged" },
      { name: "ja", value: "", action: "remove" },
    ];
    expect(metadataInputChange("dc:title", "languages", value)).toEqual({
      change: { op: "setLanguages", languages: { fr: "Nouveau", ja: null } },
    });
    value.languages.push({ name: "FR", value: "duplicate", action: "change" });
    expect(
      metadataInputChange("dc:title", "languages", value).error,
    ).toBeDefined();
  });
  test("prototype-named language changes survive review and Save serialization", () => {
    for (const operation of ["unchanged", "set"]) {
      for (const action of ["change", "remove"]) {
        const value = input("Default text", operation);
        value.languages = [
          { name: "fr", value: "Texte", action: "change" },
          { name: "__proto__", value: "Selected text", action },
          { name: "de", value: "Keep", action: "unchanged" },
        ];
        const result = metadataInputChange("dc:title", "languages", value);
        expect(result.error).toBeUndefined();
        expect(result.change?.op).toBe("setLanguages");
        if (result.change?.op !== "setLanguages")
          throw new Error("Every selected language must produce a change.");
        const languages = result.change.languages;
        expect(Object.hasOwn(languages, "__proto__")).toBe(true);
        expect(languages["__proto__"]).toBe(
          action === "remove" ? null : "Selected text",
        );
        expect(Object.hasOwn(languages, "de")).toBe(false);
        const store = createMetadataDrafts();
        const draft = store.admit("a", facts("a"), "{}")!;
        draft.changes["dc:title"] = result.change;
        draft.reviewed = true;
        const serialized = JSON.parse(store.beginSave("a")!.body) as {
          changes: Record<string, { languages: Record<string, string | null> }>;
        };
        expect(serialized.changes["dc:title"]?.languages).toEqual({
          ...(operation === "set" ? { "x-default": "Default text" } : {}),
          fr: "Texte",
          ["__proto__"]: action === "remove" ? null : "Selected text",
        });
        value.languages.push({
          name: "__PROTO__",
          value: "Duplicate",
          action: "change",
        });
        expect(
          metadataInputChange("dc:title", "languages", value).error,
        ).toBeDefined();
      }
    }
  });
  test("stored empty default text, empty lists, and property removal differ", () => {
    expect(metadataInputChange("dc:title", "languages", input(""))).toEqual({
      change: defaultLanguageChange(""),
    });
    expect(metadataInputChange("dc:subject", "list", input(""))).toEqual({
      change: { op: "set", value: [] },
    });
    expect(
      metadataInputChange("dc:subject", "list", input("", "clear")),
    ).toEqual({ change: { op: "clear" } });
    expect(
      metadataInputChange("dc:title", "languages", input("", "remove")),
    ).toEqual({ change: { op: "remove" } });
  });
  test("ordered Creators and Keywords have separate duplicate behavior", () => {
    expect(
      metadataInputChange("dc:creator", "list", input("Second\nFirst\nSecond")),
    ).toEqual({ change: { op: "set", value: ["Second", "First", "Second"] } });
    expect(metadataInputChange("dc:subject", "list", input("a\nA\na"))).toEqual(
      { change: { op: "set", value: ["a", "A"] } },
    );
  });
  test("false, zero, fractional and rejected external ratings remain values", () => {
    expect(
      metadataInputChange("xmpRights:Marked", "boolean", input("false")),
    ).toEqual({ change: { op: "set", value: false } });
    for (const value of [0, 2.5, -1])
      expect(
        metadataInputChange("xmp:Rating", "number", input(String(value))),
      ).toEqual({ change: { op: "set", value } });
    expect(
      metadataInputChange("xmp:Rating", "number", input("")).error,
    ).toBeDefined();
    expect(
      metadataInputChange("xmpRights:Marked", "boolean", input("")).error,
    ).toBeDefined();
  });
});

describe("mounted metadata drafts", () => {
  test("Photo and Grid navigation retains raw invalid input and original evidence", () => {
    const store = createMetadataDrafts();
    const first = store.admit(
      "a",
      facts("a"),
      '{"revision":9007199254740993}',
    )!;
    first.inputs["xmp:Rating"] = input("invalid");
    first.errors["xmp:Rating"] = "Invalid rating";
    first.changes["dc:title"] = defaultLanguageChange("Draft");
    store.admit("b", facts("b"), '{"revision":2}');
    expect(store.get("a")).toBe(first);
    expect(store.get("a")?.inputs["xmp:Rating"]?.text).toBe("invalid");
    expect(store.get("a")?.evidence).toBe('{"revision":9007199254740993}');
    expect(store.changed("a", '{"revision":9007199254740993}')).toBe(false);
    expect(store.changed("a", '{"revision":9007199254740994}')).toBe(true);
    expect(first.evidence).toBe('{"revision":9007199254740993}');
    store.acceptInspection("a", facts("a"), '{"revision":3}');
    expect(first.reviewed).toBe(false);
    expect(first.changes["dc:title"]).toEqual(defaultLanguageChange("Draft"));
    expect(first.evidence).toBe('{"revision":3}');
  });
  test("pending Save owns its Photo after navigation and cannot be discarded", () => {
    const store = createMetadataDrafts();
    const first = store.admit("a", facts("a"), '{"revision":1}')!;
    first.changes["dc:title"] = defaultLanguageChange("Saved");
    first.reviewed = true;
    const admitted = store.beginSave("a")!;
    const second = store.admit("b", facts("b"), '{"revision":2}')!;
    second.changes["dc:title"] = defaultLanguageChange("Other draft");
    expect(store.discard("a")).toBe(false);
    expect(store.beginSave("a")).toBeUndefined();
    expect(JSON.parse(admitted.body)).toEqual({
      evidence: { revision: 1 },
      changes: { "dc:title": defaultLanguageChange("Saved") },
    });
    store.settle("a", admitted.draft, true);
    expect(store.get("a")).toBeUndefined();
    expect(store.get("b")).toBe(second);
    expect(second.changes["dc:title"]).toEqual(
      defaultLanguageChange("Other draft"),
    );
  });
  test("uncertain Save requires inspection and a new review without dropping intent", () => {
    const store = createMetadataDrafts();
    const draft = store.admit("a", facts("a"), "{}")!;
    draft.changes["dc:title"] = defaultLanguageChange("Maybe saved");
    draft.reviewed = true;
    const admitted = store.beginSave("a")!;
    store.settle("a", admitted.draft, false);
    draft.reviewed = true;
    expect(store.beginSave("a")).toBeUndefined();
    expect(draft.changes["dc:title"]).toEqual(
      defaultLanguageChange("Maybe saved"),
    );
    store.acceptInspection("a", facts("a"), '{"fresh":true}');
    expect(store.beginSave("a")).toBeUndefined();
    draft.reviewed = true;
    expect(store.beginSave("a")?.body).toContain('"fresh":true');
  });
  test("capacity refuses new input without silently evicting unsaved drafts", () => {
    const store = createMetadataDrafts(2);
    const a = store.admit("a", facts("a"), "a")!;
    a.changes["dc:title"] = defaultLanguageChange("Keep me");
    store.admit("b", facts("b"), "b");
    expect(store.admit("c", facts("c"), "c")).toBeUndefined();
    expect(store.get("a")).toBe(a);
    expect(store.admit("a", facts("a"), "new evidence")).toBe(a);
    store.discard("b");
    expect(store.admit("c", facts("c"), "c")).toBeDefined();
    store.clear();
    expect(store.get("a")).toBeUndefined();
  });
});
