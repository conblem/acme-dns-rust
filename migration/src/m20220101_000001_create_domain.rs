use entity::*;
use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20220101_000001_create_domain"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(domain::Entity)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(domain::Column::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(domain::Column::Username).string().not_null())
                    .col(ColumnDef::new(domain::Column::Password).string().not_null())
                    .col(ColumnDef::new(domain::Column::Txt).string())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(domain::Entity).to_owned())
            .await
    }
}
