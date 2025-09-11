import type { FaucetBalances } from '../hooks/useBalances';
import { useConfig } from '../hooks/useConfig';
import { btcToSats } from '../utils';
import { formatBTC } from '../utils';

interface FaucetBalanceProps {
  faucet: FaucetBalances;
  title: string;
}

function getHealthStatus(balanceSats: number, thresholdBtc: number): string {
  const thresholdSats = btcToSats(thresholdBtc);
  return balanceSats >= thresholdSats ? 'Healthy' : 'Low';
}

export default function FaucetBalance({ faucet, title }: FaucetBalanceProps) {
  const config = useConfig();

  const faucetWallets = [
    {
      name: 'Signet wallet',
      balance_sats: faucet.l1_balance_sats,
      threshold_btc: config.faucetBalanceBtcThresholds.signet,
    },
    {
      name: 'Alpen wallet',
      balance_sats: faucet.l2_balance_sats,
      threshold_btc: config.faucetBalanceBtcThresholds.alpen,
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
                <td className="table-cell">{formatBTC(wallet.balance_sats)}</td>
                <td className="table-cell">{wallet.threshold_btc} BTC</td>
                <td className="table-cell">
                  {getHealthStatus(wallet.balance_sats, wallet.threshold_btc)}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
