import { useQuery } from '@tanstack/react-query';
import { useConfig } from './useConfig';

export type NetworkStatus = {
  sequencer: string;
  rpc_endpoint: string;
  ee_endpoint?: string;
  bundler_endpoint: string;
  sequencer_chain?: OlChainStatus | null;
  rpc_chain?: OlChainStatus | null;
  ee_chain?: EvmChainStatus | null;
};

export type BlockInfoStatus = {
  slot: number;
  block_id: string;
  epoch: number;
  is_terminal: boolean;
};

export type EpochCommitmentStatus = {
  epoch: number;
  last_slot: number;
  last_block_id: string;
};

export type OlChainStatus = {
  tip: BlockInfoStatus;
  latest: EpochCommitmentStatus;
  confirmed: EpochCommitmentStatus;
  finalized: EpochCommitmentStatus;
  confirmation_lag_slots: number;
  finality_lag_slots: number;
  latest_slot_stale_seconds?: number | null;
};

export type EvmChainStatus = {
  latest_block_number: number;
  latest_block_stale_seconds?: number | null;
};

const fetchNetworkStatus = async (baseUrl: string): Promise<NetworkStatus> => {
  const response = await fetch(`${baseUrl}/api/status`);
  if (!response.ok) {
    throw new Error('Failed to fetch status');
  }
  return response.json();
};

export const useNetworkStatus = () => {
  const { apiBaseUrl, networkStatusRefetchIntervalS } = useConfig();

  return useQuery({
    queryKey: ['networkStatus'],
    queryFn: () => fetchNetworkStatus(apiBaseUrl),
    refetchInterval: networkStatusRefetchIntervalS * 1000, // convert to ms
  });
};
