"""
Meridian GIS API — v0.2.0
Machine-native spatial data processing. For AI agents and developers alike.

Payment: x402 micropayments in USDC on Base.
No accounts. No API keys. No credit cards.
"""
import asyncio
import json
import time
from typing import Optional

from fastapi import BackgroundTasks, FastAPI, File, Form, Header, HTTPException, Request, UploadFile, status
from slowapi import Limiter, _rate_limit_exceeded_handler
from slowapi.util import get_remote_address
from slowapi.errors import RateLimitExceeded
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import JSONResponse, Response

from app.mcp_sse import mcp_app
from app.billing.x402 import (
    OPERATION_PRICES,
    OPERATION_DESCRIPTIONS,
)
from app.billing.solana_pay import (
    build_payment_required,
    require_payment,
)
from app.config import get_settings
from app.db import init_db, log_operation
from app.jobs import (
    extend_db_schema, create_job, get_job, complete_job, fail_job, mark_running,
    run_cleanup_loop
)
from app.operations.buffer    import run_buffer
from app.operations.tiles     import run_vectorize
from app.operations.clip      import run_clip
from app.operations.combine   import run_append, run_merge, run_spatial_join
from app.operations.convert   import run_convert
from app.operations.dxf       import run_dxf_convert
from app.operations.reproject import run_reproject
from app.operations.schema    import run_schema
from app.operations.topology  import run_union, run_intersect, run_difference
from app.operations.transform import (
    run_erase,
    run_dissolve,
    run_feature_to_point,
    run_feature_to_line,
    run_feature_to_polygon,
    run_multipart_to_singlepart,
    run_add_field,
)
from app.operations.validate  import run_validate, run_repair

settings = get_settings()

# Rate limiter — keyed by IP, bypassed for internal API key
def get_key(request: Request) -> str:
    xp = request.headers.get("X-PAYMENT", "")
    if xp and xp == settings.internal_api_key:
        return "internal"
    return get_remote_address(request)

limiter = Limiter(key_func=get_key, default_limits=["60/minute"])

app = FastAPI(
    title="Node API",
    description=(
        "Machine-native spatial data processing for AI agents and developers.\n\n"
        "Convert, reproject, validate, repair, clip, analyze, and tile vector GIS data.\n\n"
        "**Payment:** $0.01 USDC per operation on Solana Mainnet. "
        "No accounts, no API keys, no subscriptions.\n\n"
        "**How it works:**\n"
        "1. Send a request — receive a `402 Payment Required` with Solana Pay details\n"
        "2. Send 0.01 USDC to the recipient address on Solana Mainnet\n"
        "3. Re-send the request with `X-PAYMENT: <transaction_signature>`\n"
        "4. Receive your processed spatial data\n\n"
        "**All endpoints available under `/v1/` (versioned) or `/` (legacy, still supported).**"
    ),
    version=settings.app_version,
    servers=[{"url": "https://nodeapi.ai", "description": "Production"}],
    contact={"name": "Node API", "url": "https://nodeapi.ai"},
    openapi_tags=[
        {"name": "Operations", "description": "Spatial data processing — $0.01 USDC per operation"},
        {"name": "Info",       "description": "Pricing, health, service info"},
    ],
)

# Versioned + legacy router — all endpoints registered on both /v1/<path> and /<path>
from fastapi import APIRouter
router = APIRouter()

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*", "X-PAYMENT", "X-PAYMENT-RESPONSE"],
    expose_headers=["X-PAYMENT-RESPONSE", "X-Meridian-Payer"],
)


app.mount("/mcp", mcp_app)
app.state.limiter = limiter
app.add_exception_handler(RateLimitExceeded, _rate_limit_exceeded_handler)

# Register all routes under /v1/ (versioned) AND / (legacy backward compat)
# Must be done AFTER all @router decorators are defined — included at bottom of file.

_startup_tasks: set = set()

@app.on_event("startup")
async def startup():
    init_db()
    extend_db_schema()
    task = asyncio.create_task(run_cleanup_loop())
    _startup_tasks.add(task)
    task.add_done_callback(_startup_tasks.discard)


# ── Info ─────────────────────────────────────────────────────────────────────

@app.get("/", include_in_schema=False)
def root():
    from fastapi.responses import HTMLResponse
    from app.billing.solana_pay import FLAT_PRICE_USD
    pricing_rows = "\n".join(
        f"<tr><td>/{op}</td><td>{OPERATION_DESCRIPTIONS.get(op, '')}</td><td>${FLAT_PRICE_USD:.2f}</td></tr>"
        for op in OPERATION_PRICES.keys()
    )
    html = f"""<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Node API — Spatial Data Processing</title>
  <style>
    * {{ box-sizing: border-box; margin: 0; padding: 0; }}
    body {{ font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
           background: #fff; color: #1a1a1a; line-height: 1.6; }}
    .wrap {{ max-width: 760px; margin: 0 auto; padding: 64px 24px; }}
    h1 {{ font-size: 1.75rem; font-weight: 600; letter-spacing: -0.02em; }}
    .sub {{ color: #555; margin-top: 8px; font-size: 1.05rem; }}
    .divider {{ border: none; border-top: 1px solid #e5e5e5; margin: 40px 0; }}
    h2 {{ font-size: 0.8rem; font-weight: 600; text-transform: uppercase;
          letter-spacing: 0.08em; color: #888; margin-bottom: 16px; }}
    table {{ width: 100%; border-collapse: collapse; font-size: 0.9rem; }}
    th {{ text-align: left; padding: 6px 12px 6px 0; color: #888;
          font-weight: 500; font-size: 0.8rem; text-transform: uppercase;
          letter-spacing: 0.05em; border-bottom: 1px solid #e5e5e5; }}
    td {{ padding: 8px 12px 8px 0; border-bottom: 1px solid #f0f0f0; vertical-align: top; }}
    td:last-child {{ font-variant-numeric: tabular-nums; color: #555; white-space: nowrap; }}
    td:first-child {{ font-family: monospace; font-size: 0.85rem; color: #1a1a1a; }}
    .links {{ display: flex; gap: 24px; margin-top: 40px; }}
    .links a {{ color: #0066cc; text-decoration: none; font-size: 0.95rem; }}
    .links a:hover {{ text-decoration: underline; }}
    .payment {{ background: #f8f8f8; border-radius: 6px; padding: 20px 24px;
                font-size: 0.9rem; color: #444; margin-top: 40px; }}
    .payment code {{ font-family: monospace; background: #eee; padding: 1px 5px;
                     border-radius: 3px; font-size: 0.85rem; }}
    .ver {{ color: #bbb; font-size: 0.8rem; margin-top: 48px; }}
  </style>
</head>
<body>
  <div class="wrap">
    <h1>Node API</h1>
    <p class="sub">Spatial data processing API. Convert, reproject, validate, repair, clip, and analyze vector GIS data.</p>

    <hr class="divider">

    <h2>Operations</h2>
    <table>
      <thead><tr><th>Endpoint</th><th>Description</th><th>Price (USDC)</th></tr></thead>
      <tbody>{pricing_rows}</tbody>
    </table>

    <div class="payment">
      <strong>Payment:</strong> $0.01 USDC per operation on Solana Mainnet. No accounts, no API keys, no subscriptions.<br>
      Send a request → receive a <code>402</code> with Solana Pay details → send USDC → resend with <code>X-PAYMENT: &lt;tx_signature&gt;</code> → receive your data.
    </div>

    <div class="links">
      <a href="/docs">API Reference →</a>
      <a href="/pricing">Pricing (JSON) →</a>
      <a href="/health">Health →</a>
    </div>

    <p class="ver">nodeapi.ai &mdash; v{settings.app_version}</p>
  </div>
</body>
</html>"""
    return HTMLResponse(content=html)

@router.get("/health", tags=["Info"], summary="Service health")
def health():
    return {"status": "ok", "version": settings.app_version}


@router.get("/pricing", tags=["Info"], summary="Operation pricing in USDC")
def pricing():
    """
    Returns per-operation pricing in USDC on Solana Mainnet.
    Flat rate: $0.01 per operation regardless of type.
    """
    from app.billing.solana_pay import FLAT_PRICE_USD, USDC_MINT
    return {
        "protocol": "solana-pay",
        "network": "mainnet-beta",
        "asset": "USDC",
        "token_mint": USDC_MINT,
        "flat_rate_usd": FLAT_PRICE_USD,
        "prices": {
            op: {
                "usd": FLAT_PRICE_USD,
                "description": desc,
            }
            for op, desc in OPERATION_DESCRIPTIONS.items()
        },
    }


# ── Helpers ──────────────────────────────────────────────────────────────────

def _file_response(out_bytes: bytes, out_filename: str, media_type: str,
                   payer: str = "") -> Response:
    headers = {"Content-Disposition": f'attachment; filename="{out_filename}"'}
    if payer:
        headers["X-Meridian-Payer"] = payer
    return Response(content=out_bytes, media_type=media_type, headers=headers)


# ── Operations ───────────────────────────────────────────────────────────────

@router.post(
    "/convert",
    tags=["Operations"],
    summary="Convert between spatial vector formats",
    description=(
        "Convert a spatial file between formats.\n\n"
        "**Supported inputs:** GeoJSON, Shapefile (.zip), KML, GeoPackage, GDB\n\n"
        "**Supported outputs:** `geojson`, `shapefile`, `kml`, `gpkg`\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof. "
        "Omit to receive payment requirements."
    ),
)
async def convert(
    request: Request,
    file: UploadFile           = File(..., description="Spatial file to convert"),
    output_format: str         = Form(..., description="Target format: geojson | shapefile | kml | gpkg"),
    input_format: Optional[str] = Form(None, description="Source format (auto-detected if omitted)"),
    x_payment: Optional[str]   = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "convert", x_payment)

    t0 = time.monotonic()
    file_bytes = await file.read()
    if len(file_bytes) > settings.max_upload_mb * 1024 * 1024:
        raise HTTPException(status_code=413, detail=f"File exceeds {settings.max_upload_mb}MB limit")

    try:
        out_bytes, out_filename, media_type = run_convert(
            file_bytes, file.filename, input_format, output_format
        )
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("convert", input_format, output_format,
                      len(file_bytes), len(out_bytes), duration_ms, True,
                      payer_address=payer, tx_hash=txhash)
        return _file_response(out_bytes, out_filename, media_type, payer)
    except ValueError as e:
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("convert", input_format, output_format,
                      len(file_bytes), 0, duration_ms, False, str(e),
                      payer_address=payer, tx_hash=txhash)
        raise HTTPException(status_code=400, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")


@router.post(
    "/reproject",
    tags=["Operations"],
    summary="Reproject spatial data to a different CRS",
    description=(
        "Transform coordinates from one CRS to another using EPSG codes.\n\n"
        "Source CRS is auto-detected from the file. Override with `source_epsg` "
        "if the file lacks a .prj or embedded CRS.\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof."
    ),
)
async def reproject(
    request: Request,
    file: UploadFile              = File(...),
    target_epsg: int              = Form(..., description="Target CRS as EPSG code, e.g. 4326"),
    source_epsg: Optional[int]    = Form(None, description="Override source CRS if file lacks .prj"),
    output_format: Optional[str]  = Form(None, description="Output format (defaults to input format)"),
    x_payment: Optional[str]      = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "reproject", x_payment)

    t0 = time.monotonic()
    file_bytes = await file.read()
    if len(file_bytes) > settings.max_upload_mb * 1024 * 1024:
        raise HTTPException(status_code=413, detail=f"File exceeds {settings.max_upload_mb}MB limit")

    try:
        out_bytes, out_filename, media_type = run_reproject(
            file_bytes, file.filename, target_epsg, source_epsg, output_format
        )
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("reproject", str(source_epsg), str(target_epsg),
                      len(file_bytes), len(out_bytes), duration_ms, True,
                      payer_address=payer, tx_hash=txhash)
        return _file_response(out_bytes, out_filename, media_type, payer)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")


@router.post(
    "/validate",
    tags=["Operations"],
    summary="Validate vector geometry — returns JSON report",
    description=(
        "Check for invalid geometries, self-intersections, and degenerate rings.\n\n"
        "**Returns:** JSON report with `total_features`, `valid_count`, `invalid_count`, "
        "and an `issues` array with per-feature descriptions.\n\n"
        "To receive a repaired file, use `POST /repair` instead.\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof."
    ),
)
async def validate(
    request: Request,
    file: UploadFile         = File(...),
    x_payment: Optional[str] = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "validate", x_payment)

    t0 = time.monotonic()
    file_bytes = await file.read()
    if len(file_bytes) > settings.max_upload_mb * 1024 * 1024:
        raise HTTPException(status_code=413, detail=f"File exceeds {settings.max_upload_mb}MB limit")

    try:
        result = run_validate(file_bytes, file.filename)
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("validate", None, None, len(file_bytes), 0,
                      duration_ms, True, payer_address=payer, tx_hash=txhash)
        return JSONResponse(content=result)
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")


@router.post(
    "/repair",
    tags=["Operations"],
    summary="Repair invalid vector geometry — returns fixed spatial file",
    description=(
        "Validate and repair geometry using `shapely.make_valid()`. "
        "Fixes self-intersections, winding order issues, and degenerate rings.\n\n"
        "**Returns:** Repaired spatial file. "
        "Validation stats (`X-Meridian-Fixed-Count`, `X-Meridian-Total-Features`) "
        "are included as response headers.\n\n"
        "For a validation report only (no file), use `POST /validate`.\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof."
    ),
)
async def repair(
    request: Request,
    file: UploadFile              = File(...),
    output_format: Optional[str]  = Form(None, description="Output format (defaults to input format)"),
    x_payment: Optional[str]      = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "repair", x_payment)

    t0 = time.monotonic()
    file_bytes = await file.read()
    if len(file_bytes) > settings.max_upload_mb * 1024 * 1024:
        raise HTTPException(status_code=413, detail=f"File exceeds {settings.max_upload_mb}MB limit")

    try:
        out_bytes, out_filename, media_type, stats = run_repair(
            file_bytes, file.filename, output_format
        )
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("repair", None, output_format, len(file_bytes), len(out_bytes),
                      duration_ms, True, payer_address=payer, tx_hash=txhash)
        headers = {
            "Content-Disposition":         f'attachment; filename="{out_filename}"',
            "X-Meridian-Total-Features":   str(stats["total_features"]),
            "X-Meridian-Fixed-Count":      str(stats["fixed_count"]),
            "X-Meridian-Payer":            payer,
        }
        return Response(content=out_bytes, media_type=media_type, headers=headers)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")


@router.post(
    "/schema",
    tags=["Operations"],
    summary="Extract attribute schema and metadata — no geometry download",
    description=(
        "Inspect a spatial file: field names and types, CRS, geometry type, "
        "feature count, and bounding box. No geometry processing — fast and cheap.\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof."
    ),
)
async def schema(
    request: Request,
    file: UploadFile         = File(...),
    x_payment: Optional[str] = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "schema", x_payment)

    t0 = time.monotonic()
    file_bytes = await file.read()
    if len(file_bytes) > settings.max_upload_mb * 1024 * 1024:
        raise HTTPException(status_code=413, detail=f"File exceeds {settings.max_upload_mb}MB limit")

    try:
        result = run_schema(file_bytes, file.filename)
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("schema", None, None, len(file_bytes), 0,
                      duration_ms, True, payer_address=payer, tx_hash=txhash)
        return JSONResponse(content=result)
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")


@router.post(
    "/clip",
    tags=["Operations"],
    summary="Clip spatial data to a bounding box or polygon mask",
    description=(
        "Clip features to a spatial extent.\n\n"
        "- `bbox`: JSON array `[minx, miny, maxx, maxy]` in the file's native CRS\n"
        "- `mask`: GeoJSON geometry string (Polygon or MultiPolygon, WGS84)\n\n"
        "One of `bbox` or `mask` is required.\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof."
    ),
)
async def clip(
    request: Request,
    file: UploadFile              = File(...),
    bbox: Optional[str]           = Form(None, description='JSON array: [minx, miny, maxx, maxy]'),
    mask: Optional[str]           = Form(None, description="GeoJSON Polygon geometry string"),
    output_format: Optional[str]  = Form(None),
    x_payment: Optional[str]      = Header(None, alias="X-PAYMENT"),
):
    import json as _json
    payer, txhash = await require_payment(request, "clip", x_payment)

    t0 = time.monotonic()
    file_bytes = await file.read()
    if len(file_bytes) > settings.max_upload_mb * 1024 * 1024:
        raise HTTPException(status_code=413, detail=f"File exceeds {settings.max_upload_mb}MB limit")

    bbox_parsed = None
    if bbox:
        try:
            bbox_parsed = _json.loads(bbox)
        except Exception:
            raise HTTPException(status_code=400, detail="bbox must be a JSON array: [minx, miny, maxx, maxy]")

    try:
        out_bytes, out_filename, media_type = run_clip(
            file_bytes, file.filename, bbox_parsed, mask, output_format
        )
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("clip", None, output_format, len(file_bytes), len(out_bytes),
                      duration_ms, True, payer_address=payer, tx_hash=txhash)
        return _file_response(out_bytes, out_filename, media_type, payer)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")

# ── DXF ──────────────────────────────────────────────────────────────────────

@router.post(
    "/dxf",
    tags=["Operations"],
    summary="Convert DXF/CAD file to vector spatial format",
    description=(
        "Extract geometry from DXF files (LINE, LWPOLYLINE, POLYLINE, CIRCLE, ARC, HATCH, SPLINE) "
        "and convert to GeoJSON, Shapefile, KML, or GeoPackage.\n\n"
        "**Important:** DXF files carry no CRS. Provide `source_epsg` if you know the "
        "coordinate system. Without it, raw DXF coordinates are preserved as-is.\n\n"
        "For large files this runs as an async job — response is `202 Accepted` "
        "with a `job_id`. Poll `GET /jobs/{job_id}` for the result.\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof."
    ),
)
async def dxf(
    request: Request,
    background_tasks: BackgroundTasks,
    file: UploadFile              = File(...),
    output_format: str            = Form(..., description="Target format: geojson | shapefile | kml | gpkg"),
    source_epsg: Optional[int]    = Form(None, description="Assign CRS to output (DXF has no CRS)"),
    layer_filter: Optional[str]   = Form(None, description="JSON array of DXF layer names to include"),
    entity_types: Optional[str]   = Form(None, description="JSON array of entity types: LINE, LWPOLYLINE, etc."),
    webhook_url: Optional[str]    = Form(None, description="POST callback URL for job completion"),
    x_payment: Optional[str]      = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "dxf", x_payment)

    file_bytes = await file.read()
    if len(file_bytes) > settings.max_upload_mb * 1024 * 1024:
        raise HTTPException(status_code=413, detail=f"File exceeds {settings.max_upload_mb}MB limit")

    layers = json.loads(layer_filter) if layer_filter else None
    etypes = json.loads(entity_types) if entity_types else None

    # Large files OR webhook requested → async job
    ASYNC_THRESHOLD = 5 * 1024 * 1024  # 5MB
    if len(file_bytes) > ASYNC_THRESHOLD or webhook_url:
        job_id = create_job("dxf", payer, txhash, webhook_url=webhook_url)

        async def _run():
            mark_running(job_id)
            t0 = time.monotonic()
            try:
                out_bytes, out_fn, mime, stats = run_dxf_convert(
                    file_bytes, file.filename, output_format, source_epsg, layers, etypes
                )
                complete_job(job_id, out_bytes, out_fn, mime, stats)
                log_operation("dxf", "dxf", output_format, len(file_bytes), len(out_bytes),
                              int((time.monotonic()-t0)*1000), True, payer_address=payer, tx_hash=txhash)
            except Exception as e:
                fail_job(job_id, str(e))
                log_operation("dxf", "dxf", output_format, len(file_bytes), 0, 0, False,
                              str(e), payer_address=payer, tx_hash=txhash)

        background_tasks.add_task(_run)
        response_body = {
            "job_id": job_id,
            "status": "pending",
            "poll_url": f"/jobs/{job_id}",
            "download_url": f"/jobs/{job_id}/download",
            "message": "Job queued. Poll /jobs/{job_id} or await webhook.",
        }
        if webhook_url:
            response_body["webhook_url"] = webhook_url
            response_body["webhook_note"] = (
                "POST will be sent to webhook_url on completion. "
                "Verify with X-Webhook-Signature header (HMAC-SHA256)."
            )
        return JSONResponse(status_code=202, content=response_body)

    # Small files → synchronous
    t0 = time.monotonic()
    try:
        out_bytes, out_fn, media_type, stats = run_dxf_convert(
            file_bytes, file.filename, output_format, source_epsg, layers, etypes
        )
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("dxf", "dxf", output_format, len(file_bytes), len(out_bytes),
                      duration_ms, True, payer_address=payer, tx_hash=txhash)
        headers = {
            "Content-Disposition":        f'attachment; filename="{out_fn}"',
            "X-Meridian-Feature-Count":   str(stats["feature_count"]),
            "X-Meridian-Layer-Count":     str(stats["layer_count"]),
            "X-Meridian-Payer":           payer,
        }
        if stats.get("warnings"):
            headers["X-Meridian-Warnings"] = "; ".join(stats["warnings"])
        return Response(content=out_bytes, media_type=media_type, headers=headers)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")


# ── Jobs ─────────────────────────────────────────────────────────────────────

@router.get(
    "/jobs/{job_id}",
    tags=["Operations"],
    summary="Poll async job status or retrieve result",
    description=(
        "Poll the status of an async job (created by large DXF conversions).\n\n"
        "- `pending` / `running` → job not ready, poll again\n"
        "- `done` → result file is returned directly as a download\n"
        "- `failed` → error detail in response body"
    ),
)
async def poll_job(job_id: str):
    job = get_job(job_id)
    if not job:
        raise HTTPException(status_code=404, detail="Job not found")

    if job["status"] in ("pending", "running"):
        return JSONResponse(content={
            "job_id": job_id,
            "status": job["status"],
            "operation": job["operation"],
            "created_at": job["created_at"].isoformat() if job["created_at"] else None,
            "webhook_url": job.get("webhook_url"),
            "webhook_delivered": job.get("webhook_delivered", False),
        })

    if job["status"] == "failed":
        return JSONResponse(
            status_code=500,
            content={"job_id": job_id, "status": "failed", "error": job["error"]}
        )

    # Done — return status + download link (not the file itself)
    meta = job.get("result_meta") or {}
    return JSONResponse(content={
        "job_id": job_id,
        "status": "done",
        "operation": job["operation"],
        "result_filename": job["result_name"],
        "result_mime": job["result_mime"],
        "size_bytes": len(bytes(job["result_bytes"] or b"")),
        "download_url": f"/jobs/{job_id}/download",
        "webhook_url": job.get("webhook_url"),
        "webhook_delivered": job.get("webhook_delivered", False),
        "meta": meta,
    })


@router.get(
    "/jobs/{job_id}/download",
    tags=["Operations"],
    summary="Download async job result file",
    description=(
        "Download the result file for a completed async job. "
        "Returns the file as an attachment. Job must be in `done` status."
    ),
)
async def download_job(job_id: str):
    job = get_job(job_id)
    if not job:
        raise HTTPException(status_code=404, detail="Job not found")
    if job["status"] != "done":
        raise HTTPException(
            status_code=409,
            detail=f"Job is {job['status']}, not done. Poll /jobs/{job_id} for status."
        )
    meta = job.get("result_meta") or {}
    headers = {
        "Content-Disposition": f'attachment; filename="{job["result_name"]}"',
        "X-Meridian-Job-Id": job_id,
    }
    for k, v in meta.items():
        headers[f"X-Meridian-{k.replace('_', '-').title()}"] = str(v)
    return Response(
        content=bytes(job["result_bytes"]),
        media_type=job["result_mime"],
        headers=headers,
    )


# ── Buffer ────────────────────────────────────────────────────────────────────

@router.post(
    "/buffer",
    tags=["Operations"],
    summary="Buffer features by distance in meters",
    description=(
        "Generate buffers around all features. Distance is in **meters**. "
        "Automatically reprojects to UTM for accuracy, then back to original CRS.\n\n"
        "Cap style: `round` | `flat` | `square`\n"
        "Join style: `round` | `mitre` | `bevel`\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof."
    ),
)
async def buffer(
    request: Request,
    file: UploadFile              = File(...),
    distance_meters: float        = Form(..., description="Buffer distance in meters"),
    output_format: Optional[str]  = Form(None),
    cap_style: str                = Form("round"),
    join_style: str               = Form("round"),
    resolution: int               = Form(16),
    source_epsg: Optional[int]    = Form(None, description="Assign CRS if file lacks one (e.g. DXF output)"),
    x_payment: Optional[str]      = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "buffer", x_payment)

    t0 = time.monotonic()
    file_bytes = await file.read()
    if len(file_bytes) > settings.max_upload_mb * 1024 * 1024:
        raise HTTPException(status_code=413, detail=f"File exceeds {settings.max_upload_mb}MB limit")

    try:
        out_bytes, out_fn, media_type = run_buffer(
            file_bytes, file.filename, distance_meters, output_format,
            cap_style, join_style, resolution, source_epsg
        )
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("buffer", None, output_format, len(file_bytes), len(out_bytes),
                      duration_ms, True, payer_address=payer, tx_hash=txhash)
        return _file_response(out_bytes, out_fn, media_type, payer)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")


# ── Topology ──────────────────────────────────────────────────────────────────

def _topo_endpoint(operation: str, run_fn, description: str):
    """Factory to reduce boilerplate for the three topology endpoints."""
    pass  # defined inline below


@router.post(
    "/union",
    tags=["Operations"],
    summary="Union — combine all features from two layers",
    description=(
        "Merge all features from `layer_a` and `layer_b` into one output. "
        "CRS of layer_b is automatically aligned to layer_a.\n\n"
        "Set `dissolve=true` to merge all geometries into a single dissolved feature.\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof."
    ),
)
async def union(
    request: Request,
    layer_a: UploadFile           = File(..., description="First spatial layer"),
    layer_b: UploadFile           = File(..., description="Second spatial layer"),
    output_format: Optional[str]  = Form(None),
    dissolve: bool                = Form(False),
    x_payment: Optional[str]      = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "union", x_payment)
    t0 = time.monotonic()
    bytes_a = await layer_a.read()
    bytes_b = await layer_b.read()
    try:
        out_bytes, out_fn, media_type, topo_stats = run_union(
            bytes_a, layer_a.filename, bytes_b, layer_b.filename, output_format, dissolve
        )
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("union", None, output_format,
                      len(bytes_a)+len(bytes_b), len(out_bytes), duration_ms, True,
                      payer_address=payer, tx_hash=txhash)
        hdrs = {"Content-Disposition": f'attachment; filename="{out_fn}"',
                "X-Meridian-Payer": payer,
                "X-Meridian-Total-Features": str(topo_stats.get("total_features", "")),
                "X-Meridian-Layer-Count": str(topo_stats.get("layer_count", 1))}
        return Response(content=out_bytes, media_type=media_type, headers=hdrs)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")


@router.post(
    "/intersect",
    tags=["Operations"],
    summary="Intersect — areas common to both layers",
    description=(
        "Return the spatial intersection of `layer_a` and `layer_b`. "
        "Attributes from layer_a are preserved. Returns 400 if no overlap exists.\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof."
    ),
)
async def intersect(
    request: Request,
    layer_a: UploadFile           = File(...),
    layer_b: UploadFile           = File(...),
    output_format: Optional[str]  = Form(None),
    x_payment: Optional[str]      = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "intersect", x_payment)
    t0 = time.monotonic()
    bytes_a = await layer_a.read()
    bytes_b = await layer_b.read()
    try:
        out_bytes, out_fn, media_type, topo_stats = run_intersect(
            bytes_a, layer_a.filename, bytes_b, layer_b.filename, output_format
        )
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("intersect", None, output_format,
                      len(bytes_a)+len(bytes_b), len(out_bytes), duration_ms, True,
                      payer_address=payer, tx_hash=txhash)
        hdrs = {"Content-Disposition": f'attachment; filename="{out_fn}"',
                "X-Meridian-Payer": payer,
                "X-Meridian-Total-Features": str(topo_stats.get("total_features", "")),
                "X-Meridian-Layer-Count": str(topo_stats.get("layer_count", 1))}
        return Response(content=out_bytes, media_type=media_type, headers=hdrs)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")


@router.post(
    "/difference",
    tags=["Operations"],
    summary="Difference — parts of layer_a not covered by layer_b",
    description=(
        "Return features in `layer_a` that do NOT overlap `layer_b`. "
        "Equivalent to: A minus (A ∩ B). Returns 400 if A is entirely covered by B.\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof."
    ),
)
async def difference(
    request: Request,
    layer_a: UploadFile           = File(...),
    layer_b: UploadFile           = File(...),
    output_format: Optional[str]  = Form(None),
    x_payment: Optional[str]      = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "difference", x_payment)
    t0 = time.monotonic()
    bytes_a = await layer_a.read()
    bytes_b = await layer_b.read()
    try:
        out_bytes, out_fn, media_type, topo_stats = run_difference(
            bytes_a, layer_a.filename, bytes_b, layer_b.filename, output_format
        )
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("difference", None, output_format,
                      len(bytes_a)+len(bytes_b), len(out_bytes), duration_ms, True,
                      payer_address=payer, tx_hash=txhash)
        hdrs = {"Content-Disposition": f'attachment; filename="{out_fn}"',
                "X-Meridian-Payer": payer,
                "X-Meridian-Total-Features": str(topo_stats.get("total_features", "")),
                "X-Meridian-Layer-Count": str(topo_stats.get("layer_count", 1))}
        return Response(content=out_bytes, media_type=media_type, headers=hdrs)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")


# ---------------------------------------------------------------------------
# Transform operations (single input)
# ---------------------------------------------------------------------------

@router.post(
    "/erase",
    tags=["Operations"],
    summary="Erase — delete all features, preserve empty schema",
    description=(
        "Remove all features from the dataset while keeping the field schema and CRS intact. "
        "Returns an empty file ready to receive new features.\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof."
    ),
)
async def erase(
    request: Request,
    file: UploadFile                = File(...),
    output_format: Optional[str]    = Form(None),
    x_payment: Optional[str]        = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "erase", x_payment)
    t0 = time.monotonic()
    file_bytes = await file.read()
    try:
        out_bytes, out_fn, media_type, stats = run_erase(file_bytes, file.filename, output_format)
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("erase", None, output_format, len(file_bytes), len(out_bytes), duration_ms, True,
                      payer_address=payer, tx_hash=txhash)
        hdrs = {"Content-Disposition": f'attachment; filename="{out_fn}"',
                "X-Meridian-Payer": payer,
                "X-Meridian-Features-Removed": str(stats.get("features_removed", ""))}
        return Response(content=out_bytes, media_type=media_type, headers=hdrs)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")


@router.post(
    "/dissolve",
    tags=["Operations"],
    summary="Dissolve — merge features by attribute field",
    description=(
        "Dissolve features that share the same value in `field`. "
        "If no field is provided, all features are dissolved into a single geometry.\n\n"
        "`aggfunc` controls how non-geometry fields are aggregated: `first`, `sum`, `mean`, `count`, `min`, `max`.\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof."
    ),
)
async def dissolve(
    request: Request,
    file: UploadFile                = File(...),
    field: Optional[str]            = Form(None),
    aggfunc: str                    = Form("first"),
    output_format: Optional[str]    = Form(None),
    x_payment: Optional[str]        = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "dissolve", x_payment)
    t0 = time.monotonic()
    file_bytes = await file.read()
    try:
        out_bytes, out_fn, media_type, stats = run_dissolve(
            file_bytes, file.filename, field, output_format, aggfunc
        )
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("dissolve", None, output_format, len(file_bytes), len(out_bytes), duration_ms, True,
                      payer_address=payer, tx_hash=txhash)
        hdrs = {"Content-Disposition": f'attachment; filename="{out_fn}"',
                "X-Meridian-Payer": payer,
                "X-Meridian-Input-Features": str(stats.get("input_features", "")),
                "X-Meridian-Output-Features": str(stats.get("output_features", ""))}
        return Response(content=out_bytes, media_type=media_type, headers=hdrs)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")


@router.post(
    "/feature-to-point",
    tags=["Operations"],
    summary="Feature to Point — convert geometries to centroid points",
    description=(
        "Convert polygon or line features to their centroid points. "
        "All attributes are preserved. Point geometries pass through unchanged.\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof."
    ),
)
async def feature_to_point(
    request: Request,
    file: UploadFile                = File(...),
    output_format: Optional[str]    = Form(None),
    x_payment: Optional[str]        = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "feature-to-point", x_payment)
    t0 = time.monotonic()
    file_bytes = await file.read()
    try:
        out_bytes, out_fn, media_type, stats = run_feature_to_point(
            file_bytes, file.filename, output_format
        )
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("feature-to-point", None, output_format, len(file_bytes), len(out_bytes), duration_ms, True,
                      payer_address=payer, tx_hash=txhash)
        hdrs = {"Content-Disposition": f'attachment; filename="{out_fn}"',
                "X-Meridian-Payer": payer,
                "X-Meridian-Output-Features": str(stats.get("output_features", ""))}
        return Response(content=out_bytes, media_type=media_type, headers=hdrs)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")


@router.post(
    "/feature-to-line",
    tags=["Operations"],
    summary="Feature to Line — extract polygon boundaries as lines",
    description=(
        "Convert polygon features to their boundary lines. "
        "All attributes are preserved. Line/Point geometries pass through unchanged.\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof."
    ),
)
async def feature_to_line(
    request: Request,
    file: UploadFile                = File(...),
    output_format: Optional[str]    = Form(None),
    x_payment: Optional[str]        = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "feature-to-line", x_payment)
    t0 = time.monotonic()
    file_bytes = await file.read()
    try:
        out_bytes, out_fn, media_type, stats = run_feature_to_line(
            file_bytes, file.filename, output_format
        )
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("feature-to-line", None, output_format, len(file_bytes), len(out_bytes), duration_ms, True,
                      payer_address=payer, tx_hash=txhash)
        hdrs = {"Content-Disposition": f'attachment; filename="{out_fn}"',
                "X-Meridian-Payer": payer,
                "X-Meridian-Output-Features": str(stats.get("output_features", ""))}
        return Response(content=out_bytes, media_type=media_type, headers=hdrs)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")


@router.post(
    "/feature-to-polygon",
    tags=["Operations"],
    summary="Feature to Polygon — convert closed lines to polygons",
    description=(
        "Polygonize closed line geometries into polygon features using Shapely. "
        "Only closed rings produce output — open lines are discarded.\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof."
    ),
)
async def feature_to_polygon(
    request: Request,
    file: UploadFile                = File(...),
    output_format: Optional[str]    = Form(None),
    x_payment: Optional[str]        = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "feature-to-polygon", x_payment)
    t0 = time.monotonic()
    file_bytes = await file.read()
    try:
        out_bytes, out_fn, media_type, stats = run_feature_to_polygon(
            file_bytes, file.filename, output_format
        )
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("feature-to-polygon", None, output_format, len(file_bytes), len(out_bytes), duration_ms, True,
                      payer_address=payer, tx_hash=txhash)
        hdrs = {"Content-Disposition": f'attachment; filename="{out_fn}"',
                "X-Meridian-Payer": payer,
                "X-Meridian-Output-Features": str(stats.get("output_polygons", ""))}
        return Response(content=out_bytes, media_type=media_type, headers=hdrs)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")


@router.post(
    "/multipart-to-singlepart",
    tags=["Operations"],
    summary="Multipart to Single Part — explode multipart geometries",
    description=(
        "Explode MultiPolygon, MultiLineString, and MultiPoint features into individual "
        "single-part features. Attributes are duplicated for each part. CRS is preserved.\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof."
    ),
)
async def multipart_to_singlepart(
    request: Request,
    file: UploadFile                = File(...),
    output_format: Optional[str]    = Form(None),
    x_payment: Optional[str]        = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "multipart-to-singlepart", x_payment)
    t0 = time.monotonic()
    file_bytes = await file.read()
    try:
        out_bytes, out_fn, media_type, stats = run_multipart_to_singlepart(
            file_bytes, file.filename, output_format
        )
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("multipart-to-singlepart", None, output_format, len(file_bytes), len(out_bytes), duration_ms, True,
                      payer_address=payer, tx_hash=txhash)
        hdrs = {"Content-Disposition": f'attachment; filename="{out_fn}"',
                "X-Meridian-Payer": payer,
                "X-Meridian-Input-Features": str(stats.get("input_features", "")),
                "X-Meridian-Output-Features": str(stats.get("output_features", ""))}
        return Response(content=out_bytes, media_type=media_type, headers=hdrs)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")


@router.post(
    "/add-field",
    tags=["Operations"],
    summary="Add Field — add a new attribute column to all features",
    description=(
        "Add a new field to every feature in the dataset. "
        "`field_type` must be one of: `str`, `int`, `float`, `bool`. "
        "`default_value` is optional — omit for null. "
        "Returns 400 if the field already exists.\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof."
    ),
)
async def add_field(
    request: Request,
    file: UploadFile                = File(...),
    field_name: str                 = Form(...),
    field_type: str                 = Form("str"),
    default_value: Optional[str]    = Form(None),
    output_format: Optional[str]    = Form(None),
    x_payment: Optional[str]        = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "add-field", x_payment)
    t0 = time.monotonic()
    file_bytes = await file.read()
    try:
        out_bytes, out_fn, media_type, stats = run_add_field(
            file_bytes, file.filename, field_name, field_type, default_value, output_format
        )
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("add-field", None, output_format, len(file_bytes), len(out_bytes), duration_ms, True,
                      payer_address=payer, tx_hash=txhash)
        hdrs = {"Content-Disposition": f'attachment; filename="{out_fn}"',
                "X-Meridian-Payer": payer,
                "X-Meridian-Field-Added": stats.get("field_added", "")}
        return Response(content=out_bytes, media_type=media_type, headers=hdrs)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")


# ---------------------------------------------------------------------------
# Combine operations (two-input)
# ---------------------------------------------------------------------------

@router.post(
    "/append",
    tags=["Operations"],
    summary="Append — add features from layer_b into layer_a's schema",
    description=(
        "Append all features from `layer_b` onto `layer_a`. "
        "The output schema matches `layer_a` — extra fields in layer_b are dropped, "
        "missing fields are filled with null. CRS is auto-aligned to layer_a.\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof."
    ),
)
async def append(
    request: Request,
    layer_a: UploadFile             = File(..., description="Target layer (schema source)"),
    layer_b: UploadFile             = File(..., description="Source layer to append from"),
    output_format: Optional[str]    = Form(None),
    x_payment: Optional[str]        = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "append", x_payment)
    t0 = time.monotonic()
    bytes_a = await layer_a.read()
    bytes_b = await layer_b.read()
    try:
        out_bytes, out_fn, media_type, stats = run_append(
            bytes_a, layer_a.filename, bytes_b, layer_b.filename, output_format
        )
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("append", None, output_format, len(bytes_a)+len(bytes_b), len(out_bytes), duration_ms, True,
                      payer_address=payer, tx_hash=txhash)
        hdrs = {"Content-Disposition": f'attachment; filename="{out_fn}"',
                "X-Meridian-Payer": payer,
                "X-Meridian-Total-Features": str(stats.get("total_features", ""))}
        return Response(content=out_bytes, media_type=media_type, headers=hdrs)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")


@router.post(
    "/merge",
    tags=["Operations"],
    summary="Merge — combine two layers preserving all fields from both",
    description=(
        "Merge `layer_a` and `layer_b` into one dataset. Unlike Append, Merge preserves "
        "all fields from both layers (union of schemas). Missing fields are filled with null. "
        "CRS is auto-aligned to layer_a.\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof."
    ),
)
async def merge(
    request: Request,
    layer_a: UploadFile             = File(...),
    layer_b: UploadFile             = File(...),
    output_format: Optional[str]    = Form(None),
    x_payment: Optional[str]        = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "merge", x_payment)
    t0 = time.monotonic()
    bytes_a = await layer_a.read()
    bytes_b = await layer_b.read()
    try:
        out_bytes, out_fn, media_type, stats = run_merge(
            bytes_a, layer_a.filename, bytes_b, layer_b.filename, output_format
        )
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("merge", None, output_format, len(bytes_a)+len(bytes_b), len(out_bytes), duration_ms, True,
                      payer_address=payer, tx_hash=txhash)
        hdrs = {"Content-Disposition": f'attachment; filename="{out_fn}"',
                "X-Meridian-Payer": payer,
                "X-Meridian-Total-Features": str(stats.get("total_features", ""))}
        return Response(content=out_bytes, media_type=media_type, headers=hdrs)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")


@router.post(
    "/spatial-join",
    tags=["Operations"],
    summary="Spatial Join — join attributes from layer_b onto layer_a by location",
    description=(
        "Join attributes from `layer_b` onto `layer_a` based on spatial relationship.\n\n"
        "- `how`: `left` (keep all of layer_a), `inner` (only matching), `right` (keep all of layer_b)\n"
        "- `predicate`: `intersects`, `within`, `contains`, `crosses`, `touches`, `overlaps`, `nearest`\n\n"
        "Conflicting field names from layer_b are suffixed with `_right`.\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid USDC payment proof."
    ),
)
async def spatial_join(
    request: Request,
    layer_a: UploadFile             = File(..., description="Target layer (receives joined attributes)"),
    layer_b: UploadFile             = File(..., description="Join layer (attributes source)"),
    how: str                        = Form("left"),
    predicate: str                  = Form("intersects"),
    output_format: Optional[str]    = Form(None),
    x_payment: Optional[str]        = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "spatial-join", x_payment)
    t0 = time.monotonic()
    bytes_a = await layer_a.read()
    bytes_b = await layer_b.read()
    try:
        out_bytes, out_fn, media_type, stats = run_spatial_join(
            bytes_a, layer_a.filename, bytes_b, layer_b.filename, how, predicate, output_format
        )
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("spatial-join", None, output_format, len(bytes_a)+len(bytes_b), len(out_bytes), duration_ms, True,
                      payer_address=payer, tx_hash=txhash)
        hdrs = {"Content-Disposition": f'attachment; filename="{out_fn}"',
                "X-Meridian-Payer": payer,
                "X-Meridian-Joined-Features": str(stats.get("joined_features", ""))}
        return Response(content=out_bytes, media_type=media_type, headers=hdrs)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")


# ---------------------------------------------------------------------------
# Vector tiles
# ---------------------------------------------------------------------------

@router.post(
    "/vectorize",
    tags=["Operations"],
    summary="Vectorize — generate .mbtiles vector tile package",
    description=(
        "Convert a spatial file into a Mapbox Vector Tiles (.mbtiles) package using tippecanoe.\n\n"
        "The output is a self-contained SQLite file compatible with Mapbox GL JS, MapLibre, "
        "GDAL, martin, tileserver-gl, and Protomaps. Host it yourself — we generate and hand off.\n\n"
        "- `layer_name`: name of the vector layer inside the tiles (default: `data`)\n"
        "- `min_zoom` / `max_zoom`: tile zoom range, 0–16 (default: 0–14)\n"
        "- `simplify`: auto-thin features at low zooms (recommended, default: true)\n"
        "- `name` / `description`: metadata embedded in the .mbtiles file\n\n"
        "**Payment:** Include `X-PAYMENT` header with valid Solana USDC transaction signature."
    ),
)
async def vectorize(
    request: Request,
    file: UploadFile                = File(...),
    layer_name: str                 = Form("data"),
    min_zoom: int                   = Form(0),
    max_zoom: int                   = Form(14),
    simplify: bool                  = Form(True),
    name: Optional[str]             = Form(None),
    description: Optional[str]      = Form(None),
    x_payment: Optional[str]        = Header(None, alias="X-PAYMENT"),
):
    payer, txhash = await require_payment(request, "vectorize", x_payment)
    t0 = time.monotonic()
    file_bytes = await file.read()
    if len(file_bytes) > settings.max_upload_mb * 1024 * 1024:
        raise HTTPException(status_code=413, detail=f"File exceeds {settings.max_upload_mb}MB limit")
    try:
        out_bytes, out_fn, stats = run_vectorize(
            file_bytes, file.filename,
            layer_name=layer_name,
            min_zoom=min_zoom,
            max_zoom=max_zoom,
            simplify=simplify,
            tileset_name=name or layer_name,
            description=description or "",
        )
        duration_ms = int((time.monotonic() - t0) * 1000)
        log_operation("vectorize", None, "mbtiles", len(file_bytes), len(out_bytes),
                      duration_ms, True, payer_address=payer, tx_hash=txhash)
        hdrs = {
            "Content-Disposition":          f'attachment; filename="{out_fn}"',
            "X-Meridian-Payer":             payer,
            "X-Meridian-Input-Features":    str(stats.get("input_features", "")),
            "X-Meridian-Min-Zoom":          str(stats.get("min_zoom", "")),
            "X-Meridian-Max-Zoom":          str(stats.get("max_zoom", "")),
            "X-Meridian-Layer-Name":        stats.get("layer_name", ""),
            "X-Meridian-Size-Bytes":        str(stats.get("size_bytes", "")),
        }
        return Response(content=out_bytes, media_type="application/x-sqlite3", headers=hdrs)
    except ValueError as e:
        raise HTTPException(status_code=400, detail=str(e))
    except RuntimeError as e:
        raise HTTPException(status_code=500, detail=str(e))
    except Exception as e:
        raise HTTPException(status_code=500, detail=f"Processing error: {e}")



# ── Batch API ─────────────────────────────────────────────────────────────────
import base64, io

@router.post("/batch", tags=["Operations"], summary="Batch multiple operations in one payment")
@limiter.limit("5/minute")
async def batch(request: Request, x_payment: Optional[str] = Header(None, alias="X-PAYMENT")):
    """
    Run up to 10 operations in one request with a single Solana payment.

    **Body (JSON):**
    ```json
    {
      "operations": [
        {"op": "validate", "file": "<base64-encoded file>", "filename": "data.gpkg", "params": {}},
        {"op": "reproject", "file": "<base64>", "filename": "data.gpkg", "params": {"target_crs": "EPSG:4326"}}
      ]
    }
    ```

    **Payment:** $0.01 USDC × number of operations. One Solana transaction covers the batch.
    """
    body = await request.json()
    ops = body.get("operations", [])
    if not ops or len(ops) > 10:
        raise HTTPException(status_code=400, detail="Provide 1–10 operations per batch")

    from app.billing.solana_pay import FLAT_PRICE_USD, verify_solana_payment
    total_price = FLAT_PRICE_USD * len(ops)

    # Verify single payment covering full batch cost
    if x_payment and x_payment == settings.internal_api_key:
        payer, txhash = "internal", "internal"
    else:
        try:
            payer, txhash = await verify_solana_payment(x_payment, total_price)
        except Exception:
            from app.billing.solana_pay import build_payment_required
            pr = build_payment_required("batch", total_price)
            return JSONResponse(status_code=402, content=pr)

    results = []
    for item in ops:
        op = item.get("op", "")
        file_b64 = item.get("file", "")
        filename = item.get("filename", "data.gpkg")
        params = item.get("params", {})
        try:
            file_bytes = base64.b64decode(file_b64)
            if op == "validate":
                from app.operations.validate import run_validate
                result_bytes, _, _ = run_validate(file_bytes, filename)
                results.append({"op": op, "success": True, "data": base64.b64encode(result_bytes).decode(), "error": None})
            elif op == "repair":
                from app.operations.validate import run_repair
                result_bytes, _, _, _ = run_repair(file_bytes, filename, params.get("output_format", None))
                results.append({"op": op, "success": True, "data": base64.b64encode(result_bytes).decode(), "error": None})
            elif op == "reproject":
                from app.operations.reproject import run_reproject
                target_crs = params.get("target_crs", "EPSG:4326")
                target_epsg = int(str(target_crs).replace("EPSG:", "").replace("epsg:", ""))
                source_crs = params.get("source_crs")
                source_epsg = int(str(source_crs).replace("EPSG:", "").replace("epsg:", "")) if source_crs else None
                result_bytes, _, _ = run_reproject(file_bytes, filename, target_epsg, source_epsg, params.get("output_format", None))
                results.append({"op": op, "success": True, "data": base64.b64encode(result_bytes).decode(), "error": None})
            elif op == "convert":
                from app.operations.convert import run_convert
                result_bytes, _, _ = run_convert(file_bytes, filename, params.get("input_format", None), params.get("output_format", "gpkg"))
                results.append({"op": op, "success": True, "data": base64.b64encode(result_bytes).decode(), "error": None})
            elif op == "buffer":
                from app.operations.buffer import run_buffer
                result_bytes, _, _ = run_buffer(
                    file_bytes, filename,
                    float(params.get("distance", 100)),
                    params.get("output_format", None),
                    params.get("cap_style", "round"),
                    params.get("join_style", "round"),
                    int(params.get("resolution", 16)),
                    int(params.get("source_epsg")) if params.get("source_epsg") else None,
                )
                results.append({"op": op, "success": True, "data": base64.b64encode(result_bytes).decode(), "error": None})
            elif op == "dissolve":
                from app.operations.transform import run_dissolve
                result_bytes, _, _, _ = run_dissolve(file_bytes, filename, params.get("field"), params.get("output_format", None), params.get("aggfunc", "first"))
                results.append({"op": op, "success": True, "data": base64.b64encode(result_bytes).decode(), "error": None})
            else:
                results.append({"op": op, "success": False, "data": None, "error": f"Unsupported op in batch: {op}"})
        except Exception as e:
            results.append({"op": op, "success": False, "data": None, "error": str(e)})

    log_operation("batch", None, None, 0, 0, 0, True, payer_address=payer, tx_hash=txhash)
    return JSONResponse({"results": results, "ops_count": len(ops), "total_paid_usd": total_price})

# ── Router registration ───────────────────────────────────────────────────────
# Register all endpoints at both /v1/<path> and /<path> (legacy compat)
app.include_router(router, prefix="/v1")
app.include_router(router)
