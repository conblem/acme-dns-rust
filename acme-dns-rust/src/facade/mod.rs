use anyhow::Result;
use async_trait::async_trait;
use entity::domain;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveModelTrait, ActiveValue, DatabaseConnection, EntityTrait, IntoActiveModel, NotSet, Set,
    Value,
};

struct DomainFacadeImpl {
    pool: DatabaseConnection,
}

#[async_trait]
trait DomainFacade {
    async fn all(&self) -> Vec<domain::Model>;
    async fn by_id(&self, id: Uuid) -> Option<domain::Model>;
    async fn update(&self, domain: domain::Model) -> Result<domain::Model>;
}

#[async_trait]
impl DomainFacade for DomainFacadeImpl {
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
    use entity::domain;
    use rstest::*;
    use sea_orm::prelude::Uuid;
    use sea_orm::{DatabaseBackend, MockDatabase};

    use crate::facade::{DomainFacade, DomainFacadeImpl};

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
}
