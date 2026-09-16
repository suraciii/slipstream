/**
 * Reshapes the normalized Capture Time for display.
 *
 * Capture Time is camera-local time with no timezone, so this must stay a
 * string reshape. Parsing it into a `Date` would apply the browser's local
 * timezone and change the recorded camera time. An unrecognized value is
 * shown unchanged rather than replaced with an invented time.
 */
export function formatCaptureTime(value: string): string {
  const match = /^(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2})/.exec(value);
  return match ? `${match[1]} ${match[2]}` : value;
}
