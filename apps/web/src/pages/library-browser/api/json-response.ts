export type JsonResponse =
  | Readonly<{ kind: "ok"; value: unknown }>
  | Readonly<{ kind: "rejected"; status: number; transport?: true }>
  | Readonly<{ kind: "malformed" }>;

export async function fetchJson(
  request: () => Promise<Response>,
): Promise<JsonResponse> {
  let response: Response;
  try {
    response = await request();
  } catch {
    return Object.freeze({ kind: "rejected", status: 0, transport: true });
  }
  if (!response.ok)
    return Object.freeze({ kind: "rejected", status: response.status });
  try {
    const value: unknown = await response.json();
    return Object.freeze({ kind: "ok", value });
  } catch {
    return Object.freeze({ kind: "malformed" });
  }
}
