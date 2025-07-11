import { lazy, Suspense, useState } from "react";
import { Link, useLocation } from "react-router-dom";
import { useNetworkStatus } from "../hooks/useNetworkStatus";
import { usePaymasterWallets } from "../hooks/usePaymasterWallets";
import convertWeiToBtc from "../utils";
import "../styles/network.css";

const StatusCard = lazy(() => import("../components/StatusCard"));
const BalanceCard = lazy(() => import("../components/BalanceCard"));
const Bridge = lazy(() => import("./Bridge"));
const Activity = lazy(() => import("./Activity"));

export default function Dashboard() {
    const [isMenuOpen, setMenuOpen] = useState(false);
    const toggleMenu = () => {
        setMenuOpen((prev) => !prev);
    };
    const { pathname } = useLocation(); // Get current URL path
    const { data, isLoading, error } = useNetworkStatus();
    const {
        data: wallets,
        isLoading: bal_isLoading,
        error: bal_error,
    } = usePaymasterWallets();

    return (
        <div className="dashboard">
            <div className="sidebar">
                {/* Logo Wrapper */}
                <a href="/" className="logo-wrapper">
                    <div className="logo-svg">
                        <img src="/alpen-logo.svg" alt="ALPEN" />
                    </div>
                </a>
                {/* Hamburger / Cross toggle — only shown on mobile */}
                <div className="menu-button" onClick={toggleMenu}>
                    {isMenuOpen ? (
                        <div className="cross">
                            <div className="cross-bar"></div>
                            <div className="cross-bar"></div>
                        </div>
                    ) : (
                        <div className="hamburger">
                            <div className="hamburger-bar"></div>
                            <div className="hamburger-bar"></div>
                            <div className="hamburger-bar"></div>
                        </div>
                    )}
                </div>

                {/* Responsive menu dropdown (mobile) */}
                <div
                    className={`navbar-menu-wrapper ${isMenuOpen ? "show-menu" : ""}`}
                >
                    <Link
                        to="/"
                        className={`menu-item ${pathname === "/" ? "active" : ""}`}
                        onClick={() => setMenuOpen(false)}
                    >
                        Network
                    </Link>
                    <Link
                        to="/bridge"
                        className={`menu-item ${pathname === "/bridge" ? "active" : ""}`}
                        onClick={() => setMenuOpen(false)}
                    >
                        Bridge
                    </Link>
                    <Link
                        to="/activity"
                        className={`menu-item ${pathname === "/activity" ? "active" : ""}`}
                        onClick={() => setMenuOpen(false)}
                    >
                        Activity
                    </Link>
                </div>
            </div>

            <div className="content">
                {/* Network Monitor Page */}
                {pathname === "/" && (
                    <div>
                        {error && (
                            <p className="error-text">Error loading data</p>
                        )}

                        <Suspense
                            fallback={
                                <p className="loading-text">Loading...</p>
                            }
                        >
                            {isLoading ? (
                                <p className="loading-text">Loading...</p>
                            ) : (
                                <div className="status-cards">
                                    <StatusCard
                                        title="Sequencer status"
                                        status={
                                            data?.sequencer.toUpperCase() ??
                                            "Unknown"
                                        }
                                    />
                                    <StatusCard
                                        title="RPC endpoint status"
                                        status={
                                            data?.rpc_endpoint.toUpperCase() ??
                                            "Unknown"
                                        }
                                    />
                                    <StatusCard
                                        title="Bundler endpoint status"
                                        status={
                                            data?.bundler_endpoint.toUpperCase() ??
                                            "Unknown"
                                        }
                                    />
                                </div>
                            )}
                        </Suspense>
                    </div>
                )}

                {/* Paymaster Wallets Section */}
                {pathname === "/" && (
                    <div>
                        {bal_error && (
                            <p className="error-text">
                                Error loading Paymaster Wallets
                            </p>
                        )}
                        <Suspense
                            fallback={
                                <p className="loading-text">
                                    Loading paymaster balances...
                                </p>
                            }
                        >
                            {bal_isLoading ? (
                                <p className="loading-text">
                                    Loading paymaster wallets...
                                </p>
                            ) : wallets &&
                              wallets.deposit &&
                              wallets.validating ? (
                                <div className="balance-cards">
                                    <div className="balance-section">
                                        <BalanceCard
                                            title="Deposit paymaster wallet"
                                            balance={convertWeiToBtc(
                                                wallets.deposit.balance,
                                            )}
                                        />
                                    </div>
                                    <div className="balance-section">
                                        <BalanceCard
                                            title="Validating paymaster wallet"
                                            balance={convertWeiToBtc(
                                                wallets.validating.balance,
                                            )}
                                        />
                                    </div>
                                </div>
                            ) : (
                                <p className="error-text">
                                    No Paymaster Data Available
                                </p>
                            )}
                        </Suspense>
                    </div>
                )}

                {/* Bridge Page Content */}
                {pathname === "/bridge" && (
                    <div className="bridge-content">
                        <Bridge></Bridge>
                    </div>
                )}

                {/* Activity Page Content */}
                {pathname === "/activity" && <Activity></Activity>}
            </div>
        </div>
    );
}
