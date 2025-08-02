import { useQuery } from '@tanstack/react-query';
import { useConfig } from './useConfig';

export interface BridgeOperatorWalletBalance {
  wallet_type: string;
  operator_id: string;
  operator_pk: string;
  balance_sats: number;
}

export interface FaucetBalances {
  l1_balance_sats: number;
  l2_balance_sats: number;
}

export interface BridgeOperatorBalances {
  general_wallets: BridgeOperatorWalletBalance[];
  stake_chain_wallets: BridgeOperatorWalletBalance[];
}

export interface WalletBalances {
  faucet: FaucetBalances;
  bridge_operators: BridgeOperatorBalances;
}

async function fetchBalances(apiUrl: string): Promise<WalletBalances> {
  const response = await fetch(`${apiUrl}/api/balances`);
  if (!response.ok) {
    throw new Error('Failed to fetch balances');
  }
  return response.json();
}

export function useBalances() {
  const config = useConfig();
  const refetchIntervalMs = (config.balanceRefetchIntervalS || 30) * 1000;

  return useQuery({
    queryKey: ['balances'],
    queryFn: () => fetchBalances(config.apiBaseUrl),
    refetchInterval: refetchIntervalMs,
    staleTime: refetchIntervalMs - 5000, // Consider stale 5s before refetch
  });
}
