import { expect, test } from "bun:test";
import {
  createApplicationOwner,
  type ApplicationEvent,
} from "./application-owner.js";

const response = (body: unknown): Response =>
  new Response(JSON.stringify(body), { status: 200 });

const scan = (state: string, publication?: string) => ({
  state,
  ...(publication ? { publication } : {}),
});

const flush = async (): Promise<void> => {
  for (let count = 0; count < 12; count += 1) await Promise.resolve();
};

test("a folder check sends one scoped request and reports its scope", async () => {
  const events: ApplicationEvent[] = [];
  let sent: string | undefined;
  let scanRequests = 0;
  const owner = createApplicationOwner(
    (input, init) => {
      if (input === "/api/scan") {
        scanRequests += 1;
        sent = typeof init?.body === "string" ? init.body : undefined;
        return Promise.resolve(
          response({ ...scan("recovering", "publication-1"), scope: "shoot" }),
        );
      }
      return Promise.resolve(response(scan("recovering", "publication-1")));
    },
    {
      emit: (event) => {
        events.push(event);
      },
      schedule: () => () => {},
    },
  );
  owner.requestFolderCheck("shoot");
  await flush();
  expect(scanRequests).toBe(1);
  expect(JSON.parse(sent ?? "null")).toEqual({ folder: "shoot" });
  expect(
    events.some(
      (event) => event.kind === "scan-status" && event.scan.scope === "shoot",
    ),
  ).toBe(true);
  owner.dispose();
});
