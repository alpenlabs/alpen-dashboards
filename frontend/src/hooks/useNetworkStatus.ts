import { useQuery } from '@tanstack/react-query';
import { useConfig } from './useConfig';

export type NetworkStatus = {
  sequencer: string;
  rpc_endpoint: string;
  bundler_endpoint: string;
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
