use anyhow::Result;
use serde::Deserialize;
use std::env;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub host: String,
    pub port: u16,
    pub log_level: String,
    pub database_url: Option<String>,
    /// When true, payment enforcement is bypassed.
    ///
    /// Precedence:
    /// 1. If DEV_MODE is set, that explicit value wins.
    /// 2. Otherwise, missing/empty WALLET_ADDRESS implies dev mode for backward compatibility.
    pub dev_mode: bool,
    /// Recipient wallet address for x402/Base USDC payments.
    /// When unset, the app currently treats that as dev mode unless DEV_MODE=false is set explicitly.
    pub wallet_address: Option<String>,
    /// x402 facilitator URL used for payment verification/settlement handoff.
    pub x402_facilitator_url: String,
    /// Optional API key for MCP server bypass. When set, requests presenting a matching
    /// `X-Mcp-Key` header skip x402 payment verification entirely.
    pub mcp_api_key: Option<String>,
}

impl AppConfig {
    pub fn from_env() -> Result<Self> {
        // Load .env if present (ignore missing)
        let _ = dotenvy::dotenv();

        let wallet_address = env::var("WALLET_ADDRESS")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());

        let explicit_dev_mode = env::var("DEV_MODE")
            .ok()
            .map(|v| v.trim().to_ascii_lowercase())
            .and_then(|v| match v.as_str() {
                "1" | "true" | "yes" | "on" => Some(true),
                "0" | "false" | "no" | "off" => Some(false),
                _ => None,
            });

        let dev_mode = explicit_dev_mode.unwrap_or(wallet_address.is_none());

        // Safety: if DEV_MODE is explicitly false (prod mode) but no wallet is configured,
        // refuse to start instead of silently running without a payment recipient.
        if explicit_dev_mode == Some(false) && wallet_address.is_none() {
            anyhow::bail!("DEV_MODE=false but WALLET_ADDRESS is not set; refusing to start in prod mode without a payment recipient");
        }

        Ok(Self {
            host: env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into()),
            port: env::var("PORT")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(8100),
            log_level: env::var("LOG_LEVEL").unwrap_or_else(|_| "info".into()),
            database_url: env::var("DATABASE_URL").ok(),
            dev_mode,
            wallet_address,
            x402_facilitator_url: env::var("X402_FACILITATOR_URL")
                .unwrap_or_else(|_| "https://x402.org/facilitate".into()),
            mcp_api_key: env::var("MCP_API_KEY")
                .ok()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty()),
        })
    }
}
