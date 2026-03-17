-- Operations log: records every paid GIS operation
CREATE TABLE IF NOT EXISTS operations_log (
    id              BIGSERIAL PRIMARY KEY,
    request_id      TEXT        NOT NULL,
    operation       TEXT        NOT NULL,
    file_size_bytes BIGINT      NOT NULL,
    price_usd       NUMERIC(10, 6) NOT NULL,
    tx_signature    TEXT,                          -- NULL in dev_mode
    status          TEXT        NOT NULL DEFAULT 'ok',  -- 'ok' | 'dev' | 'error'
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_operations_log_created_at ON operations_log (created_at DESC);
CREATE INDEX IF NOT EXISTS idx_operations_log_operation   ON operations_log (operation);
