import { Suspense } from "react";
import { useLocation } from "react-router-dom";
import {
    useBridgeStatus,
    OperatorStatus,
    DepositInfo,
    WithdrawalInfo,
    ReimbursementInfo,
} from "../hooks/useBridgeStatus";
import { TxidDisplay } from "../components/TransactionId";
import { useConfig } from "../hooks/useConfig";
import { truncateHex } from "../utils";
import "../styles/bridge.css";

export default function Bridge() {
    const { pathname } = useLocation(); // Get current URL path
    const { data, isLoading, error } = useBridgeStatus();
    const { bitcoinExplorerUrl, alpenExplorerUrl } = useConfig();

    return (
        <div className="bridge-content">
            {/* Bridge Status Page */}
            {pathname === "/bridge" && (
                <div>
                    {!data ||
                        (error && (
                            <p className="error-text">Error loading data</p>
                        ))}
                    <Suspense
                        fallback={<p className="loading-text">Loading...</p>}
                    >
                        {isLoading ? (
                            <p className="loading-text">Loading...</p>
                        ) : (
                            <div className="bridge-container">
                                <div className="bridge-section">
                                    <span className="bridge-title">
                                        BRIDGE OPERATOR STATUS
                                    </span>
                                    {data && data.operators.length > 0 ? (
                                        <div className="table-wrapper">
                                            <table className="operators-table">
                                                <thead>
                                                    <tr className="operators-header">
                                                        <th>Operator</th>
                                                        <th>Public key</th>
                                                        <th>Status</th>
                                                    </tr>
                                                </thead>
                                                <tbody>
                                                    {data.operators.map(
                                                        (
                                                            operator: OperatorStatus,
                                                            index: number,
                                                        ) => (
                                                            <tr
                                                                key={index}
                                                                className="operators-row"
                                                            >
                                                                <td className="table-cell">
                                                                    {
                                                                        operator.operator_id
                                                                    }
                                                                </td>
                                                                <td className="table-cell">
                                                                    {truncateHex(
                                                                        operator.operator_pk,
                                                                    )}
                                                                </td>
                                                                <td
                                                                    className={`operator-status ${operator.status.toLowerCase()}`}
                                                                >
                                                                    {operator.status.toUpperCase()}
                                                                </td>
                                                            </tr>
                                                        ),
                                                    )}
                                                </tbody>
                                            </table>
                                        </div>
                                    ) : (
                                        <p className="no-items">
                                            No bridge operators found.
                                        </p>
                                    )}
                                </div>
                                <div className="bridge-section">
                                    <span className="bridge-title">
                                        BRIDGE DEPOSIT STATUS
                                    </span>
                                    {data && data.deposits.length > 0 ? (
                                        <div className="table-wrapper">
                                            <table className="transactions-table">
                                                <thead>
                                                    <tr className="transactions-header">
                                                        <th>
                                                            Deposit Request TXID
                                                        </th>
                                                        <th>Deposit TXID</th>
                                                        <th>Status</th>
                                                    </tr>
                                                </thead>
                                                <tbody>
                                                    {data.deposits.map(
                                                        (
                                                            deposit: DepositInfo,
                                                            index: number,
                                                        ) => (
                                                            <tr
                                                                key={index}
                                                                className="transactions-row"
                                                            >
                                                                <td className="table-cell">
                                                                    <TxidDisplay
                                                                        explorerUrl={
                                                                            bitcoinExplorerUrl
                                                                        }
                                                                        txid={
                                                                            deposit.deposit_request_txid
                                                                        }
                                                                    />
                                                                </td>
                                                                <td className="table-cell">
                                                                    <TxidDisplay
                                                                        explorerUrl={
                                                                            bitcoinExplorerUrl
                                                                        }
                                                                        txid={
                                                                            deposit.deposit_txid
                                                                        }
                                                                    />
                                                                </td>
                                                                <td className="table-cell">
                                                                    {
                                                                        deposit.status
                                                                    }
                                                                </td>
                                                            </tr>
                                                        ),
                                                    )}
                                                </tbody>
                                            </table>
                                        </div>
                                    ) : (
                                        <p className="no-items">
                                            No bridge deposits found.
                                        </p>
                                    )}
                                </div>
                                <div className="bridge-section">
                                    <span className="bridge-title">
                                        BRIDGE WITHDRAWAL STATUS
                                    </span>
                                    {data && data.withdrawals.length > 0 ? (
                                        <div className="table-wrapper">
                                            <table className="transactions-table">
                                                <thead>
                                                    <tr className="transactions-header">
                                                        <th>
                                                            Withdrawal Request
                                                            TXID
                                                        </th>
                                                        <th>
                                                            Fulfillment TXID
                                                        </th>
                                                        <th>Status</th>
                                                    </tr>
                                                </thead>
                                                <tbody>
                                                    {data.withdrawals.map(
                                                        (
                                                            withdrawal: WithdrawalInfo,
                                                            index: number,
                                                        ) => (
                                                            <tr
                                                                key={index}
                                                                className="transactions-row"
                                                            >
                                                                <td className="table-cell">
                                                                    <TxidDisplay
                                                                        explorerUrl={
                                                                            alpenExplorerUrl
                                                                        }
                                                                        txid={`0x${withdrawal.withdrawal_request_txid}`}
                                                                    />
                                                                </td>
                                                                <td className="table-cell">
                                                                    <TxidDisplay
                                                                        explorerUrl={
                                                                            bitcoinExplorerUrl
                                                                        }
                                                                        txid={
                                                                            withdrawal.fulfillment_txid
                                                                        }
                                                                    />
                                                                </td>
                                                                <td className="table-cell">
                                                                    {
                                                                        withdrawal.status
                                                                    }
                                                                </td>
                                                            </tr>
                                                        ),
                                                    )}
                                                </tbody>
                                            </table>
                                        </div>
                                    ) : (
                                        <p className="no-items">
                                            No bridge withdrawals found.
                                        </p>
                                    )}
                                </div>
                                <div className="bridge-section">
                                    <span className="bridge-title">
                                        BRIDGE REIMBURSEMENT STATUS
                                    </span>
                                    {data && data.reimbursements.length > 0 ? (
                                        <div className="table-wrapper">
                                            <table className="transactions-table">
                                                <thead>
                                                    <tr className="transactions-header">
                                                        <th>Claim TXID</th>
                                                        <th>Challenge Step</th>
                                                        <th>Payout TXID</th>
                                                        <th>Status</th>
                                                    </tr>
                                                </thead>
                                                <tbody>
                                                    {data.reimbursements.map(
                                                        (
                                                            reimbursement: ReimbursementInfo,
                                                            index: number,
                                                        ) => (
                                                            <tr
                                                                key={index}
                                                                className="transactions-row"
                                                            >
                                                                <td className="table-cell">
                                                                    <TxidDisplay
                                                                        explorerUrl={
                                                                            bitcoinExplorerUrl
                                                                        }
                                                                        txid={
                                                                            reimbursement.claim_txid
                                                                        }
                                                                    />
                                                                </td>
                                                                <td className="table-cell">
                                                                    {
                                                                        reimbursement.challenge_step
                                                                    }
                                                                </td>
                                                                <td className="table-cell">
                                                                    <TxidDisplay
                                                                        explorerUrl={
                                                                            bitcoinExplorerUrl
                                                                        }
                                                                        txid={
                                                                            reimbursement.payout_txid
                                                                        }
                                                                    />
                                                                </td>
                                                                <td className="table-cell">
                                                                    {
                                                                        reimbursement.status
                                                                    }
                                                                </td>
                                                            </tr>
                                                        ),
                                                    )}
                                                </tbody>
                                            </table>
                                        </div>
                                    ) : (
                                        <p className="no-items">
                                            No bridge reimbursements found.
                                        </p>
                                    )}
                                </div>
                            </div>
                        )}
                    </Suspense>
                </div>
            )}
        </div>
    );
}
