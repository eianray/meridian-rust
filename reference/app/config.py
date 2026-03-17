from pydantic_settings import BaseSettings
from functools import lru_cache


class Settings(BaseSettings):
    # App
    app_name: str = "Meridian GIS API"
    app_version: str = "0.7.0"
    debug: bool = False

    # Database (operations log only — no accounts)
    # Set DATABASE_URL env var in production. For local dev, use a .env file.
    database_url: str = "postgresql://meridian@localhost:5432/meridian"

    # x402 / USDC on Base (legacy — kept for reference)
    wallet_address: str = "0x0000000000000000000000000000000000000000"
    x402_facilitator_url: str = "https://x402.org/facilitate"

    # Solana Pay / USDC on Solana Mainnet
    # Set solana_wallet_address to enable real payment verification.
    # Leave as default for dev mode (verification bypassed).
    solana_wallet_address: str = "YOUR_SOLANA_WALLET_ADDRESS"
    solana_rpc_url: str = "https://api.mainnet-beta.solana.com"

    # Internal API key for trusted local callers (MCP server, testing)
    # Set to a long random string. Requests with X-PAYMENT: <internal_api_key>
    # bypass Solana Pay verification entirely.
    internal_api_key: str = ""

    # File limits
    max_upload_mb: int = 50

    class Config:
        env_file = ".env"
        env_file_encoding = "utf-8"


@lru_cache()
def get_settings() -> Settings:
    return Settings()
