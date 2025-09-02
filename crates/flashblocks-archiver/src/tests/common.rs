use crate::database::Database;
use testcontainers_modules::{localstack::LocalStack, postgres::Postgres};
use uuid::Uuid;

pub struct PostgresTestContainer {
    pub _container: testcontainers::ContainerAsync<Postgres>,
    pub database: Database,
    pub database_url: String,
}

pub struct LocalStackTestContainer {
    pub _container: testcontainers::ContainerAsync<LocalStack>,
    pub s3_url: String,
}

impl PostgresTestContainer {
    pub async fn new(db_name: &str) -> anyhow::Result<Self> {
        use testcontainers::{runners::AsyncRunner, ImageExt};

        let postgres_image = Postgres::default()
            .with_db_name(db_name)
            .with_user("test_user")
            .with_password("test_password")
            .with_tag("16-alpine");

        let container = postgres_image.start().await?;
        let port = container.get_host_port_ipv4(5432).await?;

        let database_url = format!(
            "postgresql://test_user:test_password@localhost:{}/{}",
            port, db_name
        );

        let database = Database::new(&database_url, 10).await?;
        database.run_migrations().await?;

        Ok(Self {
            _container: container,
            database,
            database_url,
        })
    }

    #[allow(dead_code)]
    pub async fn create_test_builder(&self, url: &str, name: Option<&str>) -> anyhow::Result<Uuid> {
        self.database.get_or_create_builder(url, name).await
    }
}

impl LocalStackTestContainer {
    pub async fn new() -> anyhow::Result<Self> {
        use testcontainers::{runners::AsyncRunner, ImageExt};

        let localstack = LocalStack::default().with_env_var("SERVICES", "s3");
        let container = localstack.start().await?;

        let host_port = container.get_host_port_ipv4(4566).await?;
        let endpoint_url = format!("http://s3.localhost.localstack.cloud:{host_port}");

        Ok(Self {
            _container: container,
            s3_url: endpoint_url,
        })
    }
}
