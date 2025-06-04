-- Track withdrawal requests from get_ethLogs
CREATE TABLE withdrawal_requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    txid TEXT NOT NULL,
    amount INTEGER NOT NULL,
    destination TEXT NOT NULL,
    block_number INTEGER NOT NULL,
    timestamp DATETIME DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(txid, destination)
);