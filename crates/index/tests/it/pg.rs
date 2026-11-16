//! The postgres leg's shared helpers: the database URL and a scratch schema
//! that is dropped even when the test fails.

use crystalline_index::ScratchSchema;

/// The Postgres URL the parity legs run against, or `None` with a note when
/// `CRYSTALLINE_TEST_POSTGRES_URL` is unset.
pub fn pg_url() -> Option<String> {
    use std::sync::Once;
    static NOTE: Once = Once::new();
    match std::env::var("CRYSTALLINE_TEST_POSTGRES_URL") {
        Ok(u) if !u.is_empty() => Some(u),
        _ => {
            NOTE.call_once(|| {
                eprintln!(
                    "note: skipping the postgres legs (CRYSTALLINE_TEST_POSTGRES_URL is unset); turso only"
                )
            });
            None
        }
    }
}

/// A scratch schema named from `prefix`, dropped when it goes out of scope
/// even if the test fails, or `None` without a URL. Declare it before any
/// store opened in it, so it drops last.
pub fn scratch(prefix: &str) -> Option<ScratchSchema> {
    pg_url().map(|url| ScratchSchema::new(&url, prefix))
}

/// `open_store` routes a Postgres config's `schema` to the connection's
/// search path: the tables land in the scratch schema and none in `public`.
#[tokio::test]
async fn factory_opens_in_the_configured_schema() {
    use crystalline_core::config::{DatabaseBackend, DatabaseConfig};
    use sqlx::Connection;

    let Some(scratch) = scratch("factory_schema") else {
        return;
    };
    let cfg = DatabaseConfig {
        backend: DatabaseBackend::Postgres,
        url: Some(scratch.url().to_string()),
        schema: Some(scratch.name().to_string()),
    };
    let store = crystalline_index::open_store(&cfg, None, false)
        .await
        .unwrap();
    drop(store);

    let mut raw = sqlx::PgConnection::connect(scratch.url()).await.unwrap();
    let mut count = async |schema: &str| -> i64 {
        let (n,): (i64,) = sqlx::query_as(
            "SELECT count(*) FROM information_schema.tables \
             WHERE table_schema=$1 AND table_name='engram'",
        )
        .bind(schema)
        .fetch_one(&mut raw)
        .await
        .unwrap();
        n
    };
    assert_eq!(count(scratch.name()).await, 1);
    assert_eq!(count("public").await, 0);
}
