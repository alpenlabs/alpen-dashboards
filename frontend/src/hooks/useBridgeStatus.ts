import { useQuery } from '@tanstack/react-query';
import { useConfig } from './useConfig';

export type OperatorStatus = {
  operator_id: string;
  operator_pk: string;
  status: string;
};

export type DepositInfo = {
  deposit_request_txid: string;
  deposit_txid: string;
  status: string;
};

export type WithdrawalInfo = {
  withdrawal_request_txid: string;
  fulfillment_txid: string;
  status: string;
};

export type ReimbursementInfo = {
  claim_txid: string;
  challenge_step: string;
  payout_txid: string;
  status: string;
};

export type BridgeStatus = {
  operators: OperatorStatus[];
  deposits: DepositInfo[];
  withdrawals: WithdrawalInfo[];
  reimbursements: ReimbursementInfo[];
};

const fetchStatus = async (baseUrl: string): Promise<BridgeStatus> => {
  const response = await fetch(`${baseUrl}/api/bridge_status`);
  if (!response.ok) {
    throw new Error('Failed to fetch status');
  }
  return response.json();
};

export const useBridgeStatus = () => {
  const { apiBaseUrl, bridgeStatusRefetchIntervalS } = useConfig();

  return useQuery({
    queryKey: ['bridgeStatus'],
    queryFn: () => fetchStatus(apiBaseUrl),
    refetchInterval: bridgeStatusRefetchIntervalS * 1000, // convert to ms
  });
};
