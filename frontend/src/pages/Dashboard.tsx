import { lazy, Suspense, useState } from 'react';
import { Link, useLocation } from 'react-router-dom';
import type { EvmChainStatus, OlChainStatus } from '../hooks/useNetworkStatus';
import { useNetworkStatus } from '../hooks/useNetworkStatus';
import '../styles/network.css';

const StatusCard = lazy(() => import('../components/StatusCard'));
const Bridge = lazy(() => import('./Bridge'));
const Balances = lazy(() => import('./Balances'));

const formatStatus = (status?: string) => status?.toUpperCase() ?? 'UNKNOWN';

const formatNumber = (value?: number | null) =>
  value == null ? 'Unknown' : value.toLocaleString();

const formatStaleSeconds = (value?: number | null) =>
  value == null ? 'Unknown' : `${value.toLocaleString()}s`;

const formatSlotLag = (value?: number | null) =>
  value == null ? 'Unknown' : `${value.toLocaleString()} slots`;

const olChainDetails = (chain?: OlChainStatus | null) => [
  {
    label: 'Tip slot',
    value: formatNumber(chain?.tip.slot),
  },
  {
    label: 'Tip epoch',
    value: formatNumber(chain?.tip.epoch),
  },
  {
    label: 'Latest epoch',
    value:
      chain == null
        ? 'Unknown'
        : `epoch ${chain.latest.epoch} / slot ${chain.latest.last_slot.toLocaleString()}`,
  },
  {
    label: 'Confirmed',
    value:
      chain == null
        ? 'Unknown'
        : `epoch ${chain.confirmed.epoch} / slot ${chain.confirmed.last_slot.toLocaleString()}`,
  },
  {
    label: 'Finalized',
    value:
      chain == null
        ? 'Unknown'
        : `epoch ${chain.finalized.epoch} / slot ${chain.finalized.last_slot.toLocaleString()}`,
  },
  {
    label: 'Confirm lag',
    value: formatSlotLag(chain?.confirmation_lag_slots),
  },
  {
    label: 'Finality lag',
    value: formatSlotLag(chain?.finality_lag_slots),
  },
  {
    label: 'Last progressed',
    value: formatStaleSeconds(chain?.latest_slot_stale_seconds),
  },
];

const evmChainDetails = (chain?: EvmChainStatus | null) => [
  {
    label: 'Latest block',
    value: formatNumber(chain?.latest_block_number),
  },
  {
    label: 'Last progressed',
    value: formatStaleSeconds(chain?.latest_block_stale_seconds),
  },
];

export default function Dashboard() {
  const [isMenuOpen, setMenuOpen] = useState(false);
  const toggleMenu = () => {
    setMenuOpen(prev => !prev);
  };
  const { pathname } = useLocation(); // Get current URL path
  const { data, isLoading, error } = useNetworkStatus();

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
        <div className={`navbar-menu-wrapper ${isMenuOpen ? 'show-menu' : ''}`}>
          <Link
            to="/"
            className={`menu-item ${pathname === '/' ? 'active' : ''}`}
            onClick={() => setMenuOpen(false)}
          >
            Network
          </Link>
          <Link
            to="/bridge"
            className={`menu-item ${pathname === '/bridge' ? 'active' : ''}`}
            onClick={() => setMenuOpen(false)}
          >
            Bridge
          </Link>
          <Link
            to="/balances"
            className={`menu-item ${pathname === '/balances' ? 'active' : ''}`}
            onClick={() => setMenuOpen(false)}
          >
            Balances
          </Link>
        </div>
      </div>

      <div className="content">
        {/* Network Monitor Page */}
        {pathname === '/' && (
          <div>
            {error && <p className="error-text">Error loading data</p>}

            <Suspense fallback={<p className="loading-text">Loading...</p>}>
              {isLoading ? (
                <p className="loading-text">Loading...</p>
              ) : (
                <div className="network-sections">
                  <section className="network-section">
                    <h2 className="network-section-title">Service endpoints</h2>
                    <div className="status-cards">
                      <StatusCard
                        title="Sequencer"
                        status={formatStatus(data?.sequencer)}
                      />
                      <StatusCard
                        title="OL RPC"
                        status={formatStatus(data?.rpc_endpoint)}
                      />
                      <StatusCard
                        title="EE RPC"
                        status={formatStatus(data?.ee_endpoint)}
                      />
                      <StatusCard
                        title="Bundler"
                        status={formatStatus(data?.bundler_endpoint)}
                      />
                    </div>
                  </section>

                  <section className="network-section">
                    <h2 className="network-section-title">Chain production</h2>
                    <div className="status-cards">
                      <StatusCard
                        title="Sequencer OL"
                        status={formatStatus(data?.sequencer)}
                        details={olChainDetails(data?.sequencer_chain)}
                      />
                      <StatusCard
                        title="Public OL RPC"
                        status={formatStatus(data?.rpc_endpoint)}
                        details={olChainDetails(data?.rpc_chain)}
                      />
                      <StatusCard
                        title="EE blocks"
                        status={formatStatus(data?.ee_endpoint)}
                        details={evmChainDetails(data?.ee_chain)}
                      />
                    </div>
                  </section>
                </div>
              )}
            </Suspense>
          </div>
        )}

        {/* Bridge Page Content */}
        {pathname === '/bridge' && (
          <div className="bridge-content">
            <Bridge></Bridge>
          </div>
        )}

        {/* Balances Page Content */}
        {pathname === '/balances' && (
          <div className="balances-content">
            <Balances></Balances>
          </div>
        )}
      </div>
    </div>
  );
}
