// use backend::market::stonfi::client::StonfiClient;

// #[tokio::test]
// #[ignore = "hits live ston.fi API"]
// async fn fetch_stonfi_pool_and_print() -> anyhow::Result<()> {
//     let _ = tracing_subscriber::fmt::try_init();

//     let client = StonfiClient::new()?;

//     let pool = client
//         .fetch_pool("EQDxmqSzZAfVuIubSDHwI4Y0uV6hy_4sxSYeIj_UmQESukxk")
//         .await?;

//     let reserve0: u128 = pool.reserve0.parse()?;
//     let reserve1: u128 = pool.reserve1.parse()?;

//     // lp_fee is given as string; usually in basis points
//     let lp_fee_bps: u32 = pool.lp_fee.parse()?;

//     println!("pool address: {}", pool.address);
//     println!("reserves: {} / {}", reserve0, reserve1);
//     println!("lp_fee_bps: {}", lp_fee_bps);
//     println!("deprecated: {}", pool.deprecated);

//     Ok(())
// }
