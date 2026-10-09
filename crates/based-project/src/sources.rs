//! Read the discovered source set in its deterministic FileId order.
use crate::{Error, Sources};
use based_manifest::Project;

pub(super) fn read(project: &Project) -> Result<Sources, Error> {
    project
        .files
        .iter()
        .map(|file| {
            std::fs::read_to_string(&file.path)
                .map(|text| (file.path.clone(), text))
                .map_err(|source| Error::Io {
                    path: file.path.clone(),
                    source,
                })
        })
        .collect()
}
