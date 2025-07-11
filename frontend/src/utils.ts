// utils.ts

/**
 * Truncate a hex string to a short display format.
 * E.g., "0xabcdef...123456"
 */
export function truncateHex(hex: string, length: number = 6): string {
    if (hex.length <= 2 * length + 2) return hex;
    return `${hex.slice(0, length)}...${hex.slice(-length)}`;
}

/**
 * Convert Wei to BTC
 */
export default function convertWeiToBtc(wei: string): string {
    const ethInWei = BigInt(wei);
    const oneEthInWei = BigInt(10 ** 18); // 1 ETH = 10^18 Wei

    // Convert Wei to ETH (ETH = Wei / 10^18)
    const btcAmount = Number(ethInWei) / Number(oneEthInWei);

    // Format to 8 decimal places (standard BTC format)
    return btcAmount.toFixed(8);
}
