// utils.ts

/**
 * Truncate a hex string to a short display format.
 * E.g., "0xabcdef...123456"
 */
export function truncateHex(hex: string, length: number = 6): string {
    if (hex.length <= 2 * length + 2) return hex;
    return `${hex.slice(0, length)}...${hex.slice(-length)}`;
}
