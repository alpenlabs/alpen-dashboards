-- Track withdrawal requests from get_ethLogs
CREATE TABLE withdrawal_requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    txid TEXT NOT NULL,
    amount INTEGER NOT NULL,
    destination TEXT NOT NULL,
    block_number INTEGER NOT NULL,
    timestamp DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(txid, destination)
);

-- Create indexer_state table to track progress of block indexing per task
CREATE TABLE indexer_state (
    task_id TEXT PRIMARY KEY,
    last_scanned_block INTEGER NOT NULL DEFAULT 0,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Insert last scanned block for withdrawal requests
INSERT INTO indexer_state (task_id, last_scanned_block) VALUES ('withdrawal_requests', 0);
