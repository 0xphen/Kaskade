use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct PoolEnvelope {
    pub pool: Pool,
}

#[derive(Debug, Deserialize)]
pub struct Pool {
    pub address: String,

    pub reserve0: String,
    pub reserve1: String,

    pub token0_address: String,
    pub token1_address: String,

    pub lp_fee: String,
    pub protocol_fee: String,

    pub deprecated: bool,
}
