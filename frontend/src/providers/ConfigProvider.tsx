import { createContext, useState, useEffect } from 'react';
import type { PropsWithChildren } from 'react';

export interface AppConfig {
  apiBaseUrl: string;
  bitcoinExplorerUrl: string;
  alpenExplorerUrl: string;
  bridgeStatusRefetchIntervalS: number;
  networkStatusRefetchIntervalS: number;
  balanceRefetchIntervalS: number;
  environment: string;
}

const ConfigContext = createContext<AppConfig | null>(null);

export const ConfigProvider = ({ children }: PropsWithChildren) => {
  const [config, setConfig] = useState<AppConfig | null>(null);

  useEffect(() => {
    fetch('/config.json')
      .then(res => res.json())
      .then(setConfig)
      .catch(() => setConfig(null));
  }, []);

  if (!config) return <div>Loading config...</div>;

  return (
    <ConfigContext.Provider value={config}>{children}</ConfigContext.Provider>
  );
};

export default ConfigContext;
