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

trait ToChanged<T: Into<Value>> {
    fn to_set(self) -> ActiveValue<T>;
}
impl<T: Into<Value>> ToChanged<T> for ActiveValue<T> {
    fn to_set(mut self) -> ActiveValue<T> {
        match self.take() {
            Some(value) => Set(value),
            None => NotSet,
        }
    }
}
