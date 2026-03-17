-- Payment idempotency: each tx_signature may only be used once
CREATE TABLE IF NOT EXISTS used_signatures (
    tx_signature TEXT        PRIMARY KEY,
    operation    TEXT        NOT NULL,
    verified_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
