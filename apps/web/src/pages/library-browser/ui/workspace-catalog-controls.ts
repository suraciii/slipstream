import { isRecord } from "../api/guards.js";
import type { JsonObject } from "../api/workspace-edit.js";

export const objects = (value: unknown): JsonObject[] =>
  Array.isArray(value) ? value.filter(isRecord) : [];
export const object = (value: unknown): JsonObject =>
  isRecord(value) ? value : {};
export const text = (value: unknown) =>
  typeof value === "string" ? value : "";
export function node<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  content?: string,
): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag);
  if (content !== undefined) element.textContent = content;
  return element;
}
export function choice(
  label: string,
  options: [string, string][],
  current = "",
) {
  const wrapper = node("label", label),
    input = node("select");
  for (const [value, caption] of options) {
    const option = node("option", caption);
    option.value = value;
    input.append(option);
  }
  input.value = current;
  wrapper.append(input);
  return { wrapper, input };
}
export function check(label: string, initial = false) {
  const wrapper = node("label"),
    input = node("input");
  input.type = "checkbox";
  input.checked = initial;
  wrapper.append(input, document.createTextNode(label));
  return { wrapper, input };
}
