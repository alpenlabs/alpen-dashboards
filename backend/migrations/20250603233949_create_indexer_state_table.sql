-- Create indexer_state table to track progress of block indexing per task
CREATE TABLE indexer_state (
    id TEXT PRIMARY KEY,
    last_scanned_block INTEGER NOT NULL DEFAULT 0,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

-- Insert last scanned block for withdrawal requests
INSERT INTO indexer_state (id, last_scanned_block) VALUES ('withdrawal_requests', 0);
