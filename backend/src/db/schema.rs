use sqlx::AnyPool;

pub async fn migrate(pool: &AnyPool) -> anyhow::Result<()> {
    sqlx::migrate!("../migrations").run(pool).await?;

    Ok(())
}
