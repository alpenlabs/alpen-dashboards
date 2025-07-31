import React from 'react';
import { truncateHex } from '../utils';

export const TxidDisplay: React.FC<{
  explorerUrl: string;
  txid: string | null;
}> = ({ explorerUrl, txid }) => {
  if (!txid) return <>-</>;

  return (
    <span className="txidWrapper">
      <a
        href={`${explorerUrl}/tx/${txid}`}
        target="_blank"
        rel="noopener noreferrer"
        className="txidLink"
        onClick={e => e.stopPropagation()} // just in case
      >
        {truncateHex(txid)}
      </a>
    </span>
  );
};
