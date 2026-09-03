export function formatPhotoCount(count: number): string {
  return `${count.toLocaleString()} ${count === 1 ? "Photo" : "Photos"}`;
}
