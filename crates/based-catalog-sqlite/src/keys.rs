use based_catalog::{Deferral, Index, IndexOrigin, IndexTarget, Key};

pub(crate) fn primary(parts: Vec<(i64, String)>) -> Option<Key> {
    (!parts.is_empty()).then(|| Key {
        name: None,
        columns: parts.into_iter().map(|(_, name)| name).collect(),
        deferral: Deferral::NotDeferrable,
    })
}

pub(crate) fn unique(indexes: &[Index]) -> Result<Vec<Key>, sqlx::Error> {
    indexes
        .iter()
        .filter(|index| index.origin == IndexOrigin::UniqueConstraint)
        .map(|index| {
            let columns = index
                .parts
                .iter()
                .map(|part| match &part.target {
                    IndexTarget::Column(name) => Ok(name.clone()),
                    IndexTarget::Expression(_) => Err(sqlx::Error::Protocol(
                        "Unique constraint index has an unavailable column".into(),
                    )),
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Key {
                name: None,
                columns,
                deferral: Deferral::NotDeferrable,
            })
        })
        .collect()
}
