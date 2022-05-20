use entity::*;
use sea_orm_migration::prelude::*;

pub struct Migration;

impl MigrationName for Migration {
    fn name(&self) -> &str {
        "m20220520_164236_create_cert"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(cert::Entity)
                    .col(
                        ColumnDef::new(cert::Column::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(cert::Column::Update)
                            .big_integer()
                            .not_null(),
                    )
                    .col(ColumnDef::new(cert::Column::State).integer().not_null())
                    .col(ColumnDef::new(cert::Column::Cert).string())
                    .col(ColumnDef::new(cert::Column::Private).string())
                    .col(ColumnDef::new(cert::Column::DomainId).string().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("domain")
                            .from(cert::Entity, cert::Column::DomainId)
                            .to(domain::Entity, domain::Column::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(cert::Entity).to_owned())
            .await
    }
}
