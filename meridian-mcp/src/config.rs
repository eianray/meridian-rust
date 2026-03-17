pub const DEFAULT_BASE_URL: &str = "https://meridianapi.nodeapi.ai";

#[derive(Debug, Clone)]
pub struct Config {
    pub base_url: String,
    pub mcp_api_key: String,
}

impl Config {
    pub fn from_env() -> Self {
        let _ = dotenvy::dotenv();
        Config {
            base_url: std::env::var("MERIDIAN_BASE_URL")
                .unwrap_or_else(|_| DEFAULT_BASE_URL.to_string()),
            mcp_api_key: std::env::var("MCP_API_KEY").unwrap_or_default(),
        }
    }
}
