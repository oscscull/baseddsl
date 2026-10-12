//! A snapshot table as data steps see it: its columns (the elided default `id` included),
//! primary key, unique keys, and each column's value family.

use super::super::model::{ColumnSnap, Snapshot, TableSnap};
use super::super::unit::IdKind;

/// A column's value family, read off its neutral snapshot type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Fam {
    Int,
    Float,
    Decimal,
    Text,
    Bool,
    Uuid,
    Ulid,
    Timestamp,
    Date,
    Time,
    Json,
    Bytes,
    /// A string enum's wire values.
    StrEnum(Vec<String>),
    /// An int enum's wire values.
    IntEnum(Vec<String>),
    /// A `raw(…)` opaque type or an array column — not assignable from a data step.
    Opaque,
}

impl Fam {
    pub(crate) fn of(ty: &str) -> Self {
        let values = |prefix: &str| {
            ty.strip_prefix(prefix)
                .and_then(|s| s.strip_suffix(')'))
                .map(|inner| inner.split(',').map(|s| s.trim().to_string()).collect())
        };
        match ty {
            t if t.ends_with("[]") || t.starts_with("raw(") => Self::Opaque,
            "int" | "serial" => Self::Int,
            "float" => Self::Float,
            "text" => Self::Text,
            "bool" => Self::Bool,
            "uuid" => Self::Uuid,
            "ulid" => Self::Ulid,
            "timestamp" => Self::Timestamp,
            "date" => Self::Date,
            "time" => Self::Time,
            "json" => Self::Json,
            "bytes" => Self::Bytes,
            t if t.starts_with("decimal(") => Self::Decimal,
            _ => values("enum:int(")
                .map(Self::IntEnum)
                .or_else(|| values("enum(").map(Self::StrEnum))
                .unwrap_or(Self::Text),
        }
    }

    pub(crate) fn is_numeric(&self) -> bool {
        matches!(
            self,
            Self::Int | Self::Float | Self::Decimal | Self::IntEnum(_)
        )
    }

    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Int => "int",
            Self::Float => "float",
            Self::Decimal => "decimal",
            Self::Text => "text",
            Self::Bool => "bool",
            Self::Uuid => "uuid",
            Self::Ulid => "ulid",
            Self::Timestamp => "timestamp",
            Self::Date => "date",
            Self::Time => "time",
            Self::Json => "json",
            Self::Bytes => "bytes",
            Self::StrEnum(_) | Self::IntEnum(_) => "enum",
            Self::Opaque => "opaque",
        }
    }
}

/// One column of a table, the elided default `id` re-synthesized.
#[derive(Debug, Clone)]
pub(crate) struct Col {
    pub name: String,
    pub ty: String,
    pub fam: Fam,
    pub nullable: bool,
    pub has_default: bool,
    pub generated: bool,
    /// A value the database (or the engine at insert) fills when the row omits it: a
    /// default, a `serial`, or the default uuid `id`.
    pub auto: bool,
}

pub(crate) struct TableView<'a> {
    pub snap: &'a TableSnap,
    pub cols: Vec<Col>,
}

impl<'a> TableView<'a> {
    pub(crate) fn new(t: &'a TableSnap) -> Self {
        let elided_id = (!t.no_id && t.pk.is_empty() && t.column("id").is_none()).then(|| Col {
            name: "id".to_string(),
            ty: "uuid".to_string(),
            fam: Fam::Uuid,
            nullable: false,
            has_default: false,
            generated: false,
            auto: true,
        });
        let mut view = Self {
            snap: t,
            cols: t.columns.iter().map(col_of).chain(elided_id).collect(),
        };
        // A lone uuid/ulid key is minted when omitted.
        if let [pk] = view.pk().as_slice() {
            view.cols
                .iter_mut()
                .filter(|c| &c.name == pk && matches!(c.fam, Fam::Uuid | Fam::Ulid))
                .for_each(|c| c.auto = true);
        }
        view
    }

    /// The single uuid/ulid primary-key column a `create` fills with an id minted at apply
    /// time (the project's id strategy) when a row omits it.
    pub(crate) fn minted_pk(&self) -> Option<(&str, IdKind)> {
        let pk = self.pk();
        let [pk] = pk.as_slice() else {
            return None;
        };
        let c = self.col(pk).filter(|c| !c.has_default)?;
        let kind = match c.fam {
            Fam::Uuid => IdKind::Uuid,
            Fam::Ulid => IdKind::Ulid,
            _ => return None,
        };
        Some((c.name.as_str(), kind))
    }

    pub(crate) fn find(snap: &'a Snapshot, name: &str) -> Option<Self> {
        snap.table(name).map(Self::new)
    }

    pub(crate) fn name(&self) -> &str {
        &self.snap.name
    }

    pub(crate) fn col(&self, name: &str) -> Option<&Col> {
        self.cols.iter().find(|c| c.name == name)
    }

    /// The primary-key columns; empty for a keyless table.
    pub(crate) fn pk(&self) -> Vec<String> {
        match (self.snap.no_id, self.snap.pk.is_empty()) {
            (true, _) => Vec::new(),
            (false, true) => vec!["id".to_string()],
            (false, false) => self.snap.pk.clone(),
        }
    }

    /// Every unique key: the primary key, unique columns, and unique indexes.
    pub(crate) fn unique_keys(&self) -> Vec<Vec<String>> {
        let pk = Some(self.pk()).filter(|pk| !pk.is_empty());
        let unique_cols = self
            .snap
            .columns
            .iter()
            .filter(|c| c.unique)
            .map(|c| vec![c.name.clone()]);
        let unique_indexes = self
            .snap
            .indexes
            .iter()
            .filter(|i| i.unique && i.raw.is_none())
            .map(|i| i.columns.clone());
        pk.into_iter()
            .chain(unique_cols)
            .chain(unique_indexes)
            .collect()
    }

    /// Is `col` usable as a keyset walk column: unique on its own, or leading an index?
    pub(crate) fn is_walkable(&self, col: &str) -> bool {
        self.unique_keys().iter().any(|k| k == &[col.to_string()])
            || self
                .snap
                .indexes
                .iter()
                .any(|i| i.columns.first().is_some_and(|c| c == col))
    }
}

fn col_of(c: &ColumnSnap) -> Col {
    Col {
        name: c.name.clone(),
        ty: c.ty.clone(),
        fam: Fam::of(&c.ty),
        nullable: c.nullable,
        has_default: c.default.is_some(),
        generated: c.generated.is_some(),
        auto: c.default.is_some() || c.ty == "serial" || c.generated.is_some(),
    }
}
