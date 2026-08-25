export function renderApp(root: HTMLElement): void {
  const heading = document.createElement("h1");
  heading.textContent = "Slipstream";
  root.replaceChildren(heading);
}

if (typeof document !== "undefined") {
  const root = document.querySelector<HTMLElement>("#app");
  if (root) {
    renderApp(root);
  }
}
