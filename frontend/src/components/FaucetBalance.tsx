import type { FaucetBalances } from '../hooks/useBalances';

interface FaucetBalanceProps {
  faucet: FaucetBalances;
  title: string;
}

function formatSats(sats: number): string {
  if (sats === 0) return '0 SATS';

  // Add comma separators for readability
  const formattedNumber = sats.toLocaleString();
  return `${formattedNumber} SATS`;
}

export default function FaucetBalance({ faucet, title }: FaucetBalanceProps) {
  const faucetWallets = [
    { name: 'Signet wallet', balance_sats: faucet.l1_balance_sats },
    { name: 'Alpen wallet', balance_sats: faucet.l2_balance_sats },
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
            </tr>
          </thead>
          <tbody>
            {faucetWallets.map(wallet => (
              <tr key={wallet.name} className="operators-row">
                <td className="table-cell">{wallet.name}</td>
                <td className="table-cell">
                  {formatSats(wallet.balance_sats)}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
