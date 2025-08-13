import { useBalances } from '../hooks/useBalances';
import FaucetBalance from '../components/FaucetBalance';
import BridgeOperatorBalance from '../components/BridgeOperatorBalance';
import '../styles/balances.css';

export default function Balances() {
  const { data: balances, isLoading, error } = useBalances();

  if (error) {
    return <p className="error-text">Error loading balance data</p>;
  }

  if (isLoading || !balances) {
    return <p className="loading-text">Loading balances...</p>;
  }

  return (
    <div className="balances-container">
      <FaucetBalance faucet={balances.faucet} title="FAUCET WALLETS" />

      <BridgeOperatorBalance
        bridgeOperators={balances.bridge_operators}
        title="BRIDGE OPERATOR WALLETS"
      />
    </div>
  );
}
