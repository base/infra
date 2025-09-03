-- Flashblocks archiver database schema

-- Builders table - stores information about payload builders
CREATE TABLE IF NOT EXISTS builders (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    url TEXT NOT NULL UNIQUE,
    name TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Flashblocks table - stores the full flashblock messages
CREATE TABLE IF NOT EXISTS flashblocks (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    builder_id UUID NOT NULL REFERENCES builders(id),
    payload_id TEXT NOT NULL,
    flashblock_index BIGINT NOT NULL,

    block_number BIGINT NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    
    UNIQUE(builder_id, payload_id, flashblock_index)
);

-- Transactions table - stores individual transactions from flashblocks
CREATE TABLE IF NOT EXISTS transactions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),

    flashblock_id UUID NOT NULL REFERENCES flashblocks(id) ON DELETE CASCADE,
    builder_id UUID NOT NULL REFERENCES builders(id),
    payload_id TEXT NOT NULL,
    
    flashblock_index BIGINT NOT NULL,
    block_number BIGINT NOT NULL,
    
    -- Transaction data (raw bytes)
    tx_data BYTEA NOT NULL,
    tx_hash TEXT NOT NULL,
    tx_index INTEGER NOT NULL,
    
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    
    UNIQUE(flashblock_id, tx_index)
);


-- Archival job tracking tables
CREATE TABLE IF NOT EXISTS archival_jobs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    start_block BIGINT NOT NULL,
    end_block BIGINT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    s3_path TEXT,
    archived_count BIGINT DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at TIMESTAMPTZ,
    error_message TEXT,
    UNIQUE(start_block, end_block)
);

CREATE TABLE IF NOT EXISTS archival_state (
    table_name TEXT PRIMARY KEY,
    last_archived_block BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Indexes for better query performance
CREATE INDEX IF NOT EXISTS idx_flashblocks_builder_block ON flashblocks(builder_id, block_number);
CREATE INDEX IF NOT EXISTS idx_flashblocks_received_at ON flashblocks(received_at);
CREATE INDEX IF NOT EXISTS idx_flashblocks_payload_id ON flashblocks(payload_id);
CREATE INDEX IF NOT EXISTS idx_flashblocks_block_number ON flashblocks(block_number);

CREATE INDEX IF NOT EXISTS idx_transactions_flashblock_id ON transactions(flashblock_id);
CREATE INDEX IF NOT EXISTS idx_transactions_builder_block ON transactions(builder_id, block_number);
CREATE INDEX IF NOT EXISTS idx_transactions_payload_id ON transactions(payload_id);
CREATE INDEX IF NOT EXISTS idx_transactions_tx_hash ON transactions(tx_hash);

-- Archival table indexes
CREATE INDEX IF NOT EXISTS idx_archival_jobs_status ON archival_jobs(status);
CREATE INDEX IF NOT EXISTS idx_archival_jobs_blocks ON archival_jobs(start_block, end_block);