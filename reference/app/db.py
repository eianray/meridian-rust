"""
Database for Meridian — operations log only.

No accounts. No API keys. No credit ledger.
x402 handles payment; we just log what happened for analytics and debugging.
"""
from typing import Optional

import psycopg2
from psycopg2.extras import RealDictCursor

from app.config import get_settings


def get_conn():
    settings = get_settings()
    return psycopg2.connect(settings.database_url, cursor_factory=RealDictCursor)


SCHEMA = """
CREATE TABLE IF NOT EXISTS used_tx_signatures (
    signature     TEXT        PRIMARY KEY,
    operation     TEXT        NOT NULL,
    payer_address TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS operations_log (
    id            SERIAL PRIMARY KEY,
    operation     TEXT        NOT NULL,
    input_format  TEXT,
    output_format TEXT,
    input_bytes   INTEGER,
    output_bytes  INTEGER,
    duration_ms   INTEGER,
    success       BOOLEAN     NOT NULL DEFAULT TRUE,
    error         TEXT,
    payer_address TEXT,                          -- x402 payer wallet
    tx_hash       TEXT,                          -- x402 transaction hash
    network       TEXT        DEFAULT 'base',
    created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_ops_log_operation  ON operations_log(operation);
CREATE INDEX IF NOT EXISTS idx_ops_log_created_at ON operations_log(created_at);
CREATE INDEX IF NOT EXISTS idx_ops_log_payer      ON operations_log(payer_address);
"""


def init_db():
    with get_conn() as conn:
        with conn.cursor() as cur:
            cur.execute(SCHEMA)
        conn.commit()


def log_operation(
    operation: str,
    input_format: Optional[str],
    output_format: Optional[str],
    input_bytes: int,
    output_bytes: int,
    duration_ms: int,
    success: bool,
    error: Optional[str] = None,
    payer_address: Optional[str] = None,
    tx_hash: Optional[str] = None,
):
    try:
        with get_conn() as conn:
            with conn.cursor() as cur:
                cur.execute(
                    """INSERT INTO operations_log
                       (operation, input_format, output_format, input_bytes,
                        output_bytes, duration_ms, success, error, payer_address, tx_hash)
                       VALUES (%s,%s,%s,%s,%s,%s,%s,%s,%s,%s)""",
                    (operation, input_format, output_format, input_bytes,
                     output_bytes, duration_ms, success, error, payer_address, tx_hash),
                )
            conn.commit()
    except Exception:
        pass  # Never let logging break a successful operation
