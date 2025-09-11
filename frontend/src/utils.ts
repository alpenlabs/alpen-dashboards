// utils.ts

// Bitcoin conversion constants
const SATOSHIS_PER_BTC = 100_000_000;

// Convert BTC to satoshis
export const btcToSats = (btc: number): number => btc * SATOSHIS_PER_BTC;

// Convert satoshis to BTC
export const satsToBtc = (sats: number): number => sats / SATOSHIS_PER_BTC;

/**
 * Truncate a hex string to a short display format.
 * E.g., "0xabcdef...123456"
 */
export function truncateHex(hex: string, length: number = 6): string {
  if (hex.length <= 2 * length + 2) return hex;
  return `${hex.slice(0, length)}...${hex.slice(-length)}`;
}

/**
 * Format satoshis as BTC with 2 decimal places and comma separators.
 * E.g., "1,234.56 BTC"
 */
export function formatBTC(sats: number): string {
  if (sats === 0) return '0.00 BTC';

  // Convert satoshis to BTC using the utility function
  const btc = satsToBtc(sats);

  // Display as BTC with 2 decimal places and comma separators
  return `${btc.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })} BTC`;
}
