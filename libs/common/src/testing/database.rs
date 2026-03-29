//! Per-test isolated PostgreSQL database.
// Testing utilities intentionally panic on setup failure — that is always a
// test configuration error, not a recoverable runtime condition.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//!
//! Each [`TestDatabase`] instance owns a freshly-created database with all
//! migrations applied. The database is dropped (destroyed) when the value
//! goes out of scope, keeping the PostgreSQL instance clean between test runs.

use sqlx::{postgres::PgPoolOptions, PgPool};
use uuid::Uuid;

/// Error type for [`TestDatabase`] construction.
#[derive(Debug, thiserror::Error)]
pub enum TestDatabaseError {
    /// Required environment variable is missing.
    #[error("required environment variable not set: {0}")]
    EnvVar(String),

    /// SQLx operation failed.
    #[error("database error: {0}")]
    Sqlx(#[from] sqlx::Error),

    /// Migration execution failed.
    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
}

/// A per-test isolated PostgreSQL database.
///
/// Call [`TestDatabase::new()`] inside a test to obtain an exclusive database
/// instance with all migrations applied. The database is automatically removed
/// when this value is dropped.
///
/// Reads `TEST_DATABASE_URL` first, falling back to `DATABASE_URL`. The URL
/// should point to any database on the target server; the database component
/// of the URL is replaced when creating the admin connection and the test
/// database URL.
///
/// # Example
///
/// ```rust,no_run
/// # use mcp_common::testing::TestDatabase;
/// # #[tokio::main]
/// # async fn main() {
/// let db = TestDatabase::new().await.unwrap();
/// // Use db.pool for all queries — this database is only yours.
/// # }
/// ```
pub struct TestDatabase {
    /// Active connection pool for the isolated test database.
    pub pool: PgPool,
    db_name: String,
    admin_url: String,
}

impl TestDatabase {
    /// Creates a fresh isolated PostgreSQL database and runs all migrations.
    ///
    /// # Errors
    ///
    /// Returns `TestDatabaseError` if the environment variable is missing,
    /// the database cannot be created, or migrations fail.
    pub async fn new() -> Result<Self, TestDatabaseError> {
        let base_url = std::env::var("TEST_DATABASE_URL")
            .or_else(|_| std::env::var("DATABASE_URL"))
            .map_err(|_| {
                TestDatabaseError::EnvVar(
                    "TEST_DATABASE_URL or DATABASE_URL must be set for integration tests"
                        .to_string(),
                )
            })?;

        let admin_url = Self::make_admin_url(&base_url);
        let db_name = format!("test_{}", Uuid::new_v4().simple());

        // Connect to the admin DB and create the isolated test database.
        let admin_pool = PgPool::connect(&admin_url).await?;
        sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
            .execute(&admin_pool)
            .await?;
        admin_pool.close().await;

        // Connect to the new database and run all migrations.
        let test_url = Self::make_db_url(&base_url, &db_name);
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&test_url)
            .await?;

        sqlx::migrate!("../../migrations").run(&pool).await?;

        Ok(Self {
            pool,
            db_name,
            admin_url,
        })
    }

    /// Replaces the database name component of `url` with `"postgres"`.
    fn make_admin_url(url: &str) -> String {
        Self::make_db_url(url, "postgres")
    }

    /// Replaces the database name component of `url` with `new_db`.
    fn make_db_url(url: &str, new_db: &str) -> String {
        // Strip any query parameters before splitting on the last '/'.
        let (base, query) = match url.find('?') {
            Some(pos) => (&url[..pos], &url[pos..]),
            None => (url, ""),
        };
        if let Some(pos) = base.rfind('/') {
            format!("{}/{}{}", &base[..pos], new_db, query)
        } else {
            format!("{}/{}{}", base, new_db, query)
        }
    }
}

impl Drop for TestDatabase {
    fn drop(&mut self) {
        let db_name = self.db_name.clone();
        let admin_url = self.admin_url.clone();
        // Fire-and-forget: clean up in a background thread so Drop is non-blocking.
        // `WITH (FORCE)` terminates any remaining connections before dropping.
        std::thread::spawn(move || {
            if let Ok(rt) = tokio::runtime::Runtime::new() {
                rt.block_on(async move {
                    if let Ok(pool) = PgPool::connect(&admin_url).await {
                        let _ = sqlx::query(&format!(
                            "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
                        ))
                        .execute(&pool)
                        .await;
                        pool.close().await;
                    }
                });
            }
        });
    }
}
