export type RandomInteger = (maximumExclusive: number) => number;

const RANDOM_DOMAIN = 2 ** 53;

const secureRandomInteger = (maximumExclusive: number): number => {
  const words = new Uint32Array(2);
  const limit = RANDOM_DOMAIN - (RANDOM_DOMAIN % maximumExclusive);
  for (;;) {
    crypto.getRandomValues(words);
    const value = (words[0]! & 0x001f_ffff) * 0x1_0000_0000 + words[1]!;
    if (value < limit) return value % maximumExclusive;
  }
};

/// Selects one position from the complete bounded Browse Snapshot sequence.
/// The caller owns the Snapshot; this helper owns only the unbiased index rule.
export function randomPosition(
  total: number,
  randomInteger: RandomInteger = secureRandomInteger,
): number | undefined {
  if (!Number.isSafeInteger(total) || total <= 0) return undefined;
  const position = randomInteger(total);
  return Number.isSafeInteger(position) && position >= 0 && position < total
    ? position
    : undefined;
}
