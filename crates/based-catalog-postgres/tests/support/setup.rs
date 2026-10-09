use sqlx::{
    postgres::{PgConnectOptions, PgConnection},
    Connection,
};

pub struct Fixture {
    pub admin_options: PgConnectOptions,
    before: Vec<String>,
}

impl Fixture {
    pub async fn create() -> Self {
        let url = std::env::var("TEST_POSTGRES_URL")
            .expect("TEST_POSTGRES_URL is required for live catalog proof");
        let admin_options: PgConnectOptions = url.parse().unwrap();
        let mut conn = PgConnection::connect_with(&admin_options).await.unwrap();
        cleanup(&mut conn).await;
        sqlx::raw_sql(sqlx::AssertSqlSafe(include_str!("../fixture.sql")))
            .execute(&mut conn)
            .await
            .unwrap();
        let before = super::snapshot::read(&mut conn).await;
        Self {
            admin_options,
            before,
        }
    }

    pub fn reader_options(&self) -> PgConnectOptions {
        // Preserve the verified TLS settings; this fixture also runs in the TLS-required gate.
        self.admin_options
            .clone()
            .username("based_catalog_reader")
            .password("fixture_metadata_only")
    }

    pub async fn verify_no_row_access_or_changes(&self) {
        let mut reader = PgConnection::connect_with(&self.reader_options())
            .await
            .unwrap();
        for sql in [
            "SELECT * FROM \"catalog fixture\".keyless",
            "SELECT nextval('\"catalog fixture\".native_details_sequence_id_seq')",
            "INSERT INTO \"catalog fixture\".keyless VALUES ('forbidden',0)",
            "ALTER TABLE \"catalog fixture\".keyless ADD COLUMN forbidden INT",
        ] {
            assert!(
                sqlx::query(sqlx::AssertSqlSafe(sql))
                    .execute(&mut reader)
                    .await
                    .is_err(),
                "metadata account allowed {sql}"
            );
        }
        let mut admin = PgConnection::connect_with(&self.admin_options)
            .await
            .unwrap();
        assert_eq!(
            self.before,
            super::snapshot::read(&mut admin).await,
            "schema/data/sequence state changed during discovery"
        );
    }

    pub async fn remove(self) {
        let mut admin = PgConnection::connect_with(&self.admin_options)
            .await
            .unwrap();
        cleanup(&mut admin).await;
    }
}

async fn cleanup(conn: &mut PgConnection) {
    sqlx::raw_sql(sqlx::AssertSqlSafe("DROP SCHEMA IF EXISTS \"catalog fixture\" CASCADE; DROP SCHEMA IF EXISTS catalog_other CASCADE; DROP SCHEMA IF EXISTS catalog_hidden CASCADE; DROP ROLE IF EXISTS based_catalog_reader;"))
        .execute(conn).await.unwrap();
}
