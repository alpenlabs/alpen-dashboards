import {
  BridgeOperatorBalances,
  BridgeOperatorWalletBalance,
} from '../hooks/useBalances';
import { formatBTC, truncateHex } from '../utils';

interface BridgeOperatorBalanceProps {
  bridgeOperators: BridgeOperatorBalances;
  title: string;
}

export default function BridgeOperatorBalance({
  bridgeOperators,
  title,
}: BridgeOperatorBalanceProps) {
  // Create a map of operator_pk to their wallet balances
  const operatorMap = new Map<
    string,
    {
      operatorId: string;
      operatorPk: string;
      generalWalletBalance?: BridgeOperatorWalletBalance;
      stakeChainWalletBalance?: BridgeOperatorWalletBalance;
    }
  >();

  // Process general wallets
  for (const wallet of bridgeOperators.general_wallets) {
    const operatorPk = wallet.operator_pk;
    const operatorId = wallet.operator_id || 'Unknown';

    if (operatorPk) {
      const entry = {
        operatorId: operatorId,
        operatorPk: operatorPk,
        generalWalletBalance: wallet,
        stakeChainWalletBalance: undefined,
      };
      operatorMap.set(operatorPk, entry);
    }
  }

  // Process stake chain wallets
  for (const wallet of bridgeOperators.stake_chain_wallets) {
    const operatorPk = wallet.operator_pk;
    const operatorId = wallet.operator_id || 'Unknown';

    if (operatorPk) {
      if (operatorMap.has(operatorPk)) {
        // Update existing entry
        const existing = operatorMap.get(operatorPk)!;
        existing.stakeChainWalletBalance = wallet;
      } else {
        // Create new entry
        operatorMap.set(operatorPk, {
          operatorId: operatorId,
          operatorPk: operatorPk,
          generalWalletBalance: undefined,
          stakeChainWalletBalance: wallet,
        });
      }
    }
  }

  if (operatorMap.size === 0) {
    return (
      <div className="balance-section">
        <span className="balance-title">{title}</span>
        <div style={{ marginTop: '20px', fontStyle: 'italic', color: 'gray' }}>
          No bridge operator wallets found
        </div>
      </div>
    );
  }

  return (
    <div className="balance-section">
      <span className="balance-title">{title}</span>
      <div className="table-wrapper">
        <table className="operators-table">
          <thead>
            <tr className="operators-header">
              <th>Operator</th>
              <th>Public key</th>
              <th>General wallet</th>
              <th>Stake-chain wallet</th>
            </tr>
          </thead>
          <tbody>
            {Array.from(operatorMap.values()).map(operator => (
              <tr key={operator.operatorPk} className="operators-row">
                <td className="table-cell">{operator.operatorId}</td>
                <td className="table-cell">
                  {truncateHex(operator.operatorPk)}
                </td>
                <td className="table-cell">
                  {operator.generalWalletBalance
                    ? formatBTC(operator.generalWalletBalance.balance_sats)
                    : '-'}
                </td>
                <td className="table-cell">
                  {operator.stakeChainWalletBalance
                    ? formatBTC(operator.stakeChainWalletBalance.balance_sats)
                    : '-'}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
