/// Applies an installed application launch target without resetting an
/// ordinary relaunch.
///
/// `focus-existing` keeps the current Destination when the application icon
/// reactivates an open window. The browser delivers a shortcut or deep-link
/// target through `launchQueue`; that explicit target replaces the view. A
/// start-URL launch is ignored so reactivation cannot reset the Destination.
export type LaunchTargetWindow = Readonly<{
  location: Readonly<{
    href: string;
    origin: string;
    assign(url: string): void;
  }>;
  launchQueue?: Readonly<{
    setConsumer(
      consumer: (parameters: Readonly<{ targetURL?: string }>) => void,
    ): void;
  }>;
}>;

export function installLaunchTargetHandler(
  target: LaunchTargetWindow = window as unknown as LaunchTargetWindow,
  navigate?: (url: string) => void,
): void {
  const launchQueue = target.launchQueue;
  if (!launchQueue) return;
  launchQueue.setConsumer(({ targetURL }) => {
    if (!targetURL) return;
    const current = new URL(target.location.href);
    const destination = new URL(targetURL, current);
    if (destination.origin !== current.origin) return;
    if (destination.href === current.href) return;
    if (
      destination.pathname === "/" &&
      destination.search === "" &&
      destination.hash === ""
    )
      return;
    (navigate ?? ((url: string) => target.location.assign(url)))(
      destination.href,
    );
  });
}
