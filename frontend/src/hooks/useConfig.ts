import { useContext } from 'react';
import ConfigContext from '../providers/ConfigProvider';

export const useConfig = () => {
  const config = useContext(ConfigContext);
  if (!config) {
    throw new Error('useConfig must be used inside <ConfigProvider>');
  }
  return config;
};
