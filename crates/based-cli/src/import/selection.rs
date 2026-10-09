//! Parse explicit physical identities independently of model naming.
use crate::error::CliError;
use based_catalog::{Selection, TableId};

pub(super) fn parse(tables: &[String]) -> Result<Selection, CliError> {
    let identities = tables.iter().map(|table| {
        let (namespace, name) = table.split_once('.').ok_or_else(|| CliError::usage("--table needs an exact namespace.table identity (SQLite: main.table); values are not patterns"))?;
        Ok(TableId::new(namespace, name))
    }).collect::<Result<Vec<_>, CliError>>()?;
    Selection::new(identities).map_err(CliError::usage)
}
