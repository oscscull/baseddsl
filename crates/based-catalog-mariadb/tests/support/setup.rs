use sqlx::{
    mysql::{MySqlConnectOptions, MySqlConnection},
    Connection,
};

pub struct Fixture {
    admin_options: MySqlConnectOptions,
    before: Vec<String>,
}

impl Fixture {
    pub async fn create() -> Self {
        let url = std::env::var("TEST_MARIADB_URL")
            .expect("TEST_MARIADB_URL is required for live catalog proof");
        let admin_options: MySqlConnectOptions = url.parse().unwrap();
        let mut conn = MySqlConnection::connect_with(&admin_options).await.unwrap();
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

    pub fn reader_options(&self) -> MySqlConnectOptions {
        self.admin_options
            .clone()
            .username("based_catalog_reader")
            .password("fixture_metadata_only")
            .database("based_catalog_fixture")
    }

    pub async fn verify_no_row_access_or_changes(&self) {
        let mut reader = MySqlConnection::connect_with(&self.reader_options())
            .await
            .unwrap();
        for sql in [
            "SELECT * FROM based_catalog_fixture.keyless",
            "INSERT INTO based_catalog_fixture.keyless VALUES ('forbidden', 0)",
            "ALTER TABLE based_catalog_fixture.keyless ADD COLUMN forbidden INT",
        ] {
            assert!(
                sqlx::query(sqlx::AssertSqlSafe(sql))
                    .execute(&mut reader)
                    .await
                    .is_err(),
                "metadata account allowed {sql}"
            );
        }
        let mut admin = MySqlConnection::connect_with(&self.admin_options)
            .await
            .unwrap();
        assert_eq!(
            self.before,
            super::snapshot::read(&mut admin).await,
            "schema/data changed during discovery"
        );
    }

    pub async fn remove(self) {
        let mut admin = MySqlConnection::connect_with(&self.admin_options)
            .await
            .unwrap();
        cleanup(&mut admin).await;
    }
}

async fn cleanup(conn: &mut MySqlConnection) {
    sqlx::raw_sql(sqlx::AssertSqlSafe("DROP DATABASE IF EXISTS based_catalog_fixture; DROP USER IF EXISTS based_catalog_reader@'%'; DROP USER IF EXISTS based_catalog_partial@'%';"))
        .execute(conn).await.unwrap();
}
