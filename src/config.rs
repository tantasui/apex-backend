/// Runtime configuration, entirely from environment variables.
///
/// Chain-related values are optional on purpose: the apex package is not
/// deployed yet, so the server must boot and serve the read API with the
/// indexer and keeper idle.
#[derive(Debug, Clone)]
pub struct Config {
    pub port: u16,
    pub database_url: String,
    pub sui_jsonrpc_url: String,
    pub apex_package_id: Option<String>,
    pub pool_config_id: Option<String>,
    pub keeper_cap_id: Option<String>,
    pub sui_keeper_key: Option<String>,
    /// Bearer token for POST /admin/pools. Admin routes are disabled when unset.
    pub admin_secret: Option<String>,
    /// Comma-separated oracle object IDs used as the default for new pools.
    pub pool_oracle_ids: Option<Vec<String>>,
}

fn non_empty(var: &str) -> Option<String> {
    std::env::var(var).ok().filter(|v| !v.trim().is_empty())
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let port = non_empty("PORT")
            .map(|p| p.parse::<u16>())
            .transpose()?
            .unwrap_or(8080);
        let database_url = non_empty("DATABASE_URL")
            .ok_or_else(|| anyhow::anyhow!("DATABASE_URL is required"))?;
        let sui_jsonrpc_url = non_empty("SUI_JSONRPC_URL")
            .unwrap_or_else(|| "https://fullnode.testnet.sui.io:443".to_string());

        let pool_oracle_ids = non_empty("POOL_ORACLE_IDS").map(|s| {
            s.split(',')
                .map(|id| id.trim().to_string())
                .filter(|id| !id.is_empty())
                .collect()
        });

        Ok(Self {
            port,
            database_url,
            sui_jsonrpc_url,
            apex_package_id: non_empty("APEX_PACKAGE_ID"),
            pool_config_id: non_empty("POOL_CONFIG_ID"),
            keeper_cap_id: non_empty("KEEPER_CAP_ID"),
            sui_keeper_key: non_empty("SUI_KEEPER_KEY"),
            admin_secret: non_empty("ADMIN_SECRET"),
            pool_oracle_ids,
        })
    }

    pub fn indexer_enabled(&self) -> bool {
        self.apex_package_id.is_some()
    }

    pub fn keeper_enabled(&self) -> bool {
        self.apex_package_id.is_some()
            && self.pool_config_id.is_some()
            && self.keeper_cap_id.is_some()
            && self.sui_keeper_key.is_some()
    }
}
