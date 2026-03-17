"""
Async job system backed by PostgreSQL.
No Redis, no ARQ. Works fine at current scale.

Jobs are created with status=pending, a worker coroutine runs in the background,
and results are stored as bytes in the jobs table.

Clients can either:
  - Poll GET /jobs/{id} for status
  - Register a webhook_url and receive a POST when the job completes or fails

Webhook delivery:
  - POST to webhook_url with JSON payload
  - X-Webhook-Signature: sha256=<HMAC> header for verification
  - Retried up to 3 times with exponential backoff on failure
"""
import asyncio
import hashlib
import hmac
import json
import uuid
from datetime import datetime, timezone
from typing import Optional, Callable, Any

import httpx
import psycopg2
from psycopg2.extras import RealDictCursor

from app.config import get_settings

# Module-level set to hold references to background tasks, preventing GC
_background_tasks: set = set()

JOB_SCHEMA = """
CREATE TABLE IF NOT EXISTS jobs (
    id           TEXT        PRIMARY KEY,
    status       TEXT        NOT NULL DEFAULT 'pending',  -- pending|running|done|failed
    operation    TEXT        NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    payer        TEXT,
    tx_hash      TEXT,
    result_bytes BYTEA,
    result_name  TEXT,
    result_mime  TEXT,
    result_meta  JSONB,
    error        TEXT,
    webhook_url  TEXT,
    webhook_delivered BOOLEAN DEFAULT FALSE,
    expires_at   TIMESTAMPTZ NOT NULL DEFAULT NOW() + INTERVAL '24 hours'
);
CREATE INDEX IF NOT EXISTS idx_jobs_status     ON jobs(status);
CREATE INDEX IF NOT EXISTS idx_jobs_expires_at ON jobs(expires_at);
"""


def _get_conn():
    return psycopg2.connect(get_settings().database_url, cursor_factory=RealDictCursor)


def extend_db_schema():
    """Called from init_db() to add jobs table."""
    with _get_conn() as conn:
        with conn.cursor() as cur:
            cur.execute(JOB_SCHEMA)
        conn.commit()


def create_job(operation: str, payer: str, tx_hash: str,
               webhook_url: Optional[str] = None) -> str:
    """Create a pending job, return job_id."""
    job_id = str(uuid.uuid4())
    with _get_conn() as conn:
        with conn.cursor() as cur:
            cur.execute(
                """INSERT INTO jobs (id, status, operation, payer, tx_hash, webhook_url)
                   VALUES (%s, 'pending', %s, %s, %s, %s)""",
                (job_id, operation, payer, tx_hash, webhook_url)
            )
        conn.commit()
    return job_id


def get_job(job_id: str) -> Optional[dict]:
    with _get_conn() as conn:
        with conn.cursor() as cur:
            cur.execute("SELECT * FROM jobs WHERE id = %s", (job_id,))
            row = cur.fetchone()
    return dict(row) if row else None


def complete_job(job_id: str, result_bytes: bytes, result_name: str,
                 result_mime: str, meta: dict):
    with _get_conn() as conn:
        with conn.cursor() as cur:
            cur.execute(
                """UPDATE jobs SET status='done', result_bytes=%s, result_name=%s,
                   result_mime=%s, result_meta=%s, updated_at=NOW()
                   WHERE id=%s""",
                (result_bytes, result_name, result_mime,
                 json.dumps(meta), job_id)
            )
        conn.commit()
    # Fire webhook async — stored in module-level set to prevent GC
    task = asyncio.create_task(_deliver_webhook_for_job(job_id))
    _background_tasks.add(task)
    task.add_done_callback(_background_tasks.discard)


def fail_job(job_id: str, error: str):
    with _get_conn() as conn:
        with conn.cursor() as cur:
            cur.execute(
                "UPDATE jobs SET status='failed', error=%s, updated_at=NOW() WHERE id=%s",
                (error, job_id)
            )
        conn.commit()
    task = asyncio.create_task(_deliver_webhook_for_job(job_id))
    _background_tasks.add(task)
    task.add_done_callback(_background_tasks.discard)


def mark_running(job_id: str):
    with _get_conn() as conn:
        with conn.cursor() as cur:
            cur.execute(
                "UPDATE jobs SET status='running', updated_at=NOW() WHERE id=%s",
                (job_id,)
            )
        conn.commit()


def sweep_expired_jobs() -> int:
    """Delete jobs past their expires_at. Returns count deleted."""
    with _get_conn() as conn:
        with conn.cursor() as cur:
            cur.execute(
                "DELETE FROM jobs WHERE expires_at < NOW() RETURNING id"
            )
            deleted = cur.rowcount
        conn.commit()
    return deleted


async def run_cleanup_loop(interval_seconds: int = 3600):
    """Background task: sweep expired jobs every hour."""
    while True:
        await asyncio.sleep(interval_seconds)
        try:
            n = sweep_expired_jobs()
            if n:
                print(f"[jobs] swept {n} expired job(s)", flush=True)
        except Exception as e:
            print(f"[jobs] cleanup error: {e}", flush=True)


def _sign_payload(payload: bytes) -> str:
    """HMAC-SHA256 signature for webhook payload verification."""
    settings = get_settings()
    secret = settings.internal_api_key.encode() or b"nodeapi-webhook-secret"
    sig = hmac.new(secret, payload, hashlib.sha256).hexdigest()
    return f"sha256={sig}"


async def _deliver_webhook_for_job(job_id: str, max_retries: int = 3) -> None:
    """
    Fetch job from DB and POST webhook to webhook_url.
    Retries up to max_retries times with exponential backoff.
    Marks webhook_delivered=true on success.
    """
    job = get_job(job_id)
    if not job or not job.get("webhook_url"):
        return

    webhook_url = job["webhook_url"]

    # Build base URL for result download link
    base_url = "https://nodeapi.ai"

    payload = {
        "job_id": job_id,
        "operation": job.get("operation"),
        "status": job.get("status"),
        "created_at": str(job.get("created_at", "")),
        "completed_at": str(job.get("updated_at", "")),
    }

    if job.get("status") == "done":
        payload.update({
            "result_url": f"{base_url}/jobs/{job_id}/download",
            "result_filename": job.get("result_name"),
            "result_mime": job.get("result_mime"),
            "size_bytes": len(job.get("result_bytes") or b""),
            "meta": job.get("result_meta") or {},
        })
    elif job.get("status") == "failed":
        payload["error"] = job.get("error")

    body = json.dumps(payload).encode()
    signature = _sign_payload(body)
    headers = {
        "Content-Type": "application/json",
        "X-Webhook-Signature": signature,
        "X-Webhook-Job-Id": job_id,
        "User-Agent": "NodeAPI-Webhook/1.0",
    }

    for attempt in range(max_retries):
        try:
            async with httpx.AsyncClient(timeout=10.0) as client:
                resp = await client.post(webhook_url, content=body, headers=headers)
                if resp.status_code < 300:
                    # Mark delivered
                    with _get_conn() as conn:
                        with conn.cursor() as cur:
                            cur.execute(
                                "UPDATE jobs SET webhook_delivered=TRUE WHERE id=%s",
                                (job_id,)
                            )
                        conn.commit()
                    print(f"[webhook] delivered job {job_id} → {resp.status_code}", flush=True)
                    return
                else:
                    print(f"[webhook] attempt {attempt+1} failed: HTTP {resp.status_code}", flush=True)
        except Exception as e:
            print(f"[webhook] attempt {attempt+1} error: {e}", flush=True)

        # Exponential backoff: 2s, 4s, 8s
        await asyncio.sleep(2 ** (attempt + 1))

    print(f"[webhook] all retries exhausted for job {job_id}", flush=True)
