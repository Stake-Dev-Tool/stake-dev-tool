use std::time::Duration;

use sqlx::PgPool;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

/// Migrations embedded at compile time from `crates/server/migrations`.
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

const INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Builds a lazily-connected pool. `connect_lazy` never touches the network, so
/// the server can bind and serve `/healthz` even while Postgres is unreachable.
pub fn connect_lazy(database_url: &str) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new().connect_lazy(database_url)?;
    Ok(pool)
}

/// Runs pending migrations and seeds the instance identity, retrying forever
/// with exponential backoff (1s doubling, capped at 30s) so a cold Postgres
/// delays readiness without ever blocking startup. Meant to be `tokio::spawn`ed.
pub async fn run_migrations(pool: PgPool) {
    let mut backoff = INITIAL_BACKOFF;
    loop {
        match migrate(&pool).await {
            Ok(()) => {
                tracing::info!("migrations applied");
                return;
            }
            Err(e) => {
                tracing::warn!(error = %e, retry_in = ?backoff, "migration attempt failed");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

/// Applies migrations once and ensures the instance identity exists. Public so
/// tests can drive it synchronously against a real database.
pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    MIGRATOR.run(pool).await?;
    ensure_instance_id(pool).await?;
    Ok(())
}

/// Inserts a stable `instance_id` on first successful boot; a no-op afterwards.
async fn ensure_instance_id(pool: &PgPool) -> anyhow::Result<()> {
    let existing: Option<String> =
        sqlx::query_scalar("SELECT value FROM server_meta WHERE key = $1")
            .bind("instance_id")
            .fetch_optional(pool)
            .await?;
    if existing.is_none() {
        let id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO server_meta (key, value) VALUES ($1, $2) ON CONFLICT (key) DO NOTHING",
        )
        .bind("instance_id")
        .bind(&id)
        .execute(pool)
        .await?;
        tracing::info!(instance_id = %id, "seeded server instance id");
    }
    Ok(())
}
