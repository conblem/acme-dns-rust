//! SeaORM Entity. Generated by sea-orm-codegen 0.8.0

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "domain")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub username: String,
    pub password: String,
    pub txt: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::cert::Entity")]
    Cert,
}

impl Related<super::cert::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Cert.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}