use anyhow::Result;
use async_trait::async_trait;
use entity::domain;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ConnectionTrait, DatabaseConnection, EntityTrait,
    IntoActiveModel, NotSet, Set, Value,
};

struct DomainFacadeImpl<T = DatabaseConnection> {
    pool: T,
}

#[async_trait]
trait DomainFacade {
    async fn all(&self) -> Vec<domain::Model>;
    async fn by_id(&self, id: Uuid) -> Option<domain::Model>;
    async fn update(&self, domain: domain::Model) -> Result<domain::Model>;
    async fn insert(&self, domain: domain::Model) -> Result<domain::Model>;
}

#[async_trait]
impl<T: ConnectionTrait + Send> DomainFacade for DomainFacadeImpl<T> {
    async fn all(&self) -> Vec<domain::Model> {
        domain::Entity::find().all(&self.pool).await.unwrap()
    }

    async fn by_id(&self, id: Uuid) -> Option<domain::Model> {
        domain::Entity::find_by_id(id)
            .one(&self.pool)
            .await
            .unwrap()
    }

    async fn update(&self, domain: domain::Model) -> Result<domain::Model> {
        let domain = domain.into_active_model();

        domain::ActiveModel {
            id: domain.id,
            username: domain.username.to_set(),
            password: domain.password.to_set(),
            txt: domain.txt.to_set(),
        }
        .update(&self.pool)
        .await
        .map_err(Into::into)
    }

    async fn insert(&self, domain: domain::Model) -> Result<domain::Model> {
        let domain = domain.into_active_model();
        domain::ActiveModel {
            id: domain.id.to_set(),
            username: domain.username.to_set(),
            password: domain.password.to_set(),
            txt: domain.txt.to_set(),
        }
        .insert(&self.pool)
        .await
        .map_err(Into::into)
    }
}

trait ToSet<T: Into<Value>> {
    fn to_set(self) -> ActiveValue<T>;
}

impl<T: Into<Value>> ToSet<T> for ActiveValue<T> {
    fn to_set(mut self) -> ActiveValue<T> {
        match self.take() {
            Some(value) => Set(value),
            None => NotSet,
        }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use entity::domain;
    use migration::{Migrator, MigratorTrait};
    use rstest::*;
    use sea_orm::prelude::Uuid;
    use sea_orm::{
        ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, DbBackend, DbErr,
        ExecResult, MockDatabase, QueryResult, Statement,
    };
    use testcontainers::clients::Cli;
    use testcontainers::images::postgres::Postgres;
    use tokio::sync::oneshot;

    use crate::facade::{DomainFacade, DomainFacadeImpl};
    use crate::util::UnsyncRAIIRef;

    struct TestContainerConnection {
        pool: DatabaseConnection,
        _raii: UnsyncRAIIRef,
    }

    impl TestContainerConnection {
        async fn new() -> Self {
            let (port_sender, port_receiver) = oneshot::channel();
            let _raii = UnsyncRAIIRef::new(move |parker| {
                let cli = Cli::default();
                let container = cli.run(Postgres::default());
                port_sender.send(container.get_host_port(5432)).unwrap();

                parker.park()
            });

            let port = port_receiver.await.unwrap();
            let connection_string =
                format!("postgres://postgres:postgres@localhost:{}/postgres", port);
            let pool = Database::connect(connection_string).await.unwrap();

            // run migrations
            Migrator::up(&pool, None).await.unwrap();

            TestContainerConnection { pool, _raii }
        }
    }

    #[async_trait]
    impl ConnectionTrait for TestContainerConnection {
        fn get_database_backend(&self) -> DbBackend {
            self.pool.get_database_backend()
        }

        async fn execute(&self, stmt: Statement) -> Result<ExecResult, DbErr> {
            self.pool.execute(stmt).await
        }

        async fn query_one(&self, stmt: Statement) -> Result<Option<QueryResult>, DbErr> {
            self.pool.query_one(stmt).await
        }

        async fn query_all(&self, stmt: Statement) -> Result<Vec<QueryResult>, DbErr> {
            self.pool.query_all(stmt).await
        }

        fn support_returning(&self) -> bool {
            self.pool.support_returning()
        }

        fn is_mock_connection(&self) -> bool {
            self.pool.is_mock_connection()
        }
    }

    #[fixture]
    fn db() -> MockDatabase {
        MockDatabase::new(DatabaseBackend::Postgres)
    }

    #[fixture]
    fn domain_one() -> domain::Model {
        domain::Model {
            id: Uuid::from_u128(1),
            username: "Test".to_string(),
            password: "Password".to_string(),
            txt: None,
        }
    }

    #[fixture]
    fn domain_two() -> domain::Model {
        domain::Model {
            id: Uuid::from_u128(10),
            username: "username".to_string(),
            password: "tyler".to_string(),
            txt: Some("TXT is the shit".to_string()),
        }
    }

    #[rstest]
    #[tokio::test]
    async fn find_by_id(db: MockDatabase, domain_one: domain::Model, domain_two: domain::Model) {
        let pool = db
            .append_query_results(vec![vec![domain_one.clone(), domain_two], vec![]])
            .into_connection();

        let facade = DomainFacadeImpl { pool };
        let domain_res = facade.by_id(domain_one.id).await.unwrap();
        assert_eq!(domain_res, domain_one);

        let domain_res = facade.by_id(Uuid::from_u128(99)).await;
        assert_eq!(domain_res, None);
    }

    #[rstest]
    #[tokio::test]
    async fn find_all(db: MockDatabase, domain_one: domain::Model, domain_two: domain::Model) {
        let pool = db
            .append_query_results(vec![vec![domain_one.clone(), domain_two.clone()], vec![]])
            .into_connection();

        let facade = DomainFacadeImpl { pool };
        let domains = facade.all().await;
        assert_eq!(domains.len(), 2);
        assert_eq!(domains[0], domain_one);
        assert_eq!(domains[1], domain_two);

        let domains = facade.all().await;
        assert_eq!(domains.len(), 0);
    }

    // because we could test the all and by_id methods using the mock db
    // we know they are correct so we can now write tests using them
    #[rstest]
    #[tokio::test]
    async fn insert(domain_one: domain::Model, domain_two: domain::Model) {
        let pool = TestContainerConnection::new().await;
        let facade = DomainFacadeImpl { pool };

        let domain_insert = facade.insert(domain_one.clone()).await.unwrap();
        assert_eq!(domain_insert, domain_one);

        let actual = facade.by_id(domain_one.id).await.unwrap();
        assert_eq!(actual, domain_one);

        let domain_insert = facade.insert(domain_two.clone()).await.unwrap();
        assert_eq!(domain_insert, domain_two);

        let domains = facade.all().await;
        assert_eq!(domains.len(), 2);
    }
}
