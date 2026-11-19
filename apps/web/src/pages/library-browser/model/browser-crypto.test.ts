import { expect, test } from "bun:test";
import { randomUuid } from "./browser-crypto.js";

const uuid =
  /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

test("creates RFC 4122 version 4 UUIDs", () => {
  expect(randomUuid()).toMatch(uuid);
});

test("does not require the secure-context randomUUID API", () => {
  const previous = Object.getOwnPropertyDescriptor(crypto, "randomUUID");
  Object.defineProperty(crypto, "randomUUID", {
    configurable: true,
    value: undefined,
  });
  try {
    expect(randomUuid()).toMatch(uuid);
  } finally {
    if (previous) Object.defineProperty(crypto, "randomUUID", previous);
    else Reflect.deleteProperty(crypto, "randomUUID");
  }
});
