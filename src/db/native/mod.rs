pub mod application;
pub mod application_token;
pub mod audit_log;
pub mod mcp_token;
pub mod runner_manager;
pub mod user;
pub mod web_session;
pub mod workspace_runner;

use sea_orm::{
    DbBackend, EntityTrait, Iterable, Schema,
    sea_query::{Table, TableCreateStatement},
};

pub(crate) fn table_for<E>() -> TableCreateStatement
where
    E: EntityTrait,
{
    let schema = Schema::new(DbBackend::Postgres);
    let mut table = Table::create();
    table.table(E::default()).if_not_exists();
    for column in <E::Column as Iterable>::iter() {
        table.col(schema.get_column_def::<E>(column));
    }
    table.to_owned()
}

#[cfg(test)]
mod tests;
