export type ServerOptions = Readonly<{
  host: string;
  port: number;
}>;

export const defaultServerOptions: ServerOptions = {
  host: "127.0.0.1",
  port: 3000,
};

export function startServer(
  options: ServerOptions = defaultServerOptions,
): void {
  console.log(
    `Slipstream server placeholder configured for http://${options.host}:${options.port}`,
  );
}

if (import.meta.main) {
  startServer();
}
