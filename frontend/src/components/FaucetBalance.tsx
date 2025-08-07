import type { FaucetBalances } from '../hooks/useBalances';
import { useConfig } from '../hooks/useConfig';

interface FaucetBalanceProps {
  faucet: FaucetBalances;
  title: string;
}

function formatSats(sats: number): string {
  if (sats === 0) return '0 SATS';

  const formattedNumber = sats.toLocaleString('en-US');
  return `${formattedNumber} SATS`;
}

function getHealthStatus(balanceSats: number, thresholdSats: number): string {
  return balanceSats >= thresholdSats ? 'Healthy' : 'Low';
}

export default function FaucetBalance({ faucet, title }: FaucetBalanceProps) {
  const config = useConfig();

  const faucetWallets = [
    {
      name: 'Signet wallet',
      balance_sats: faucet.l1_balance_sats,
      threshold_sats: config.faucetBalanceSatsThresholds.signet,
    },
    {
      name: 'Alpen wallet',
      balance_sats: faucet.l2_balance_sats,
      threshold_sats: config.faucetBalanceSatsThresholds.alpen,
    },
  ];

  return (
    <div className="balance-section">
      <span className="balance-title">{title}</span>
      <div className="table-wrapper">
        <table className="operators-table">
          <thead>
            <tr className="operators-header">
              <th>Wallet</th>
              <th>Balance</th>
              <th>Threshold</th>
              <th>Status</th>
            </tr>
          </thead>
          <tbody>
            {faucetWallets.map(wallet => (
              <tr key={wallet.name} className="operators-row">
                <td className="table-cell">{wallet.name}</td>
                <td className="table-cell">
                  {formatSats(wallet.balance_sats)}
                </td>
                <td className="table-cell">
                  {formatSats(wallet.threshold_sats)}
                </td>
                <td className="table-cell">
                  {getHealthStatus(wallet.balance_sats, wallet.threshold_sats)}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
