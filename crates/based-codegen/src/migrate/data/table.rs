//! A snapshot table as data steps see it: its columns (the elided default `id` included),
//! primary key, unique keys, and each column's value family.

use super::super::model::{ColumnSnap, Snapshot, TableSnap};

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
        if ty.ends_with("[]") || ty.starts_with("raw(") {
            return Self::Opaque;
        }
        if let Some(inner) = ty
            .strip_prefix("enum:int(")
            .and_then(|s| s.strip_suffix(')'))
        {
            return Self::IntEnum(split_values(inner));
        }
        if let Some(inner) = ty.strip_prefix("enum(").and_then(|s| s.strip_suffix(')')) {
            return Self::StrEnum(split_values(inner));
        }
        match ty {
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
            _ => Self::Text,
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

fn split_values(inner: &str) -> Vec<String> {
    inner.split(',').map(|s| s.trim().to_string()).collect()
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
        let mut cols: Vec<Col> = t.columns.iter().map(col_of).collect();
        if !t.no_id && t.pk.is_empty() && t.column("id").is_none() {
            cols.push(Col {
                name: "id".to_string(),
                ty: "uuid".to_string(),
                fam: Fam::Uuid,
                nullable: false,
                has_default: false,
                generated: false,
                auto: true,
            });
        }
        let mut view = Self { snap: t, cols };
        if let [pk] = view.pk().as_slice() {
            if let Some(c) = view.cols.iter_mut().find(|c| &c.name == pk) {
                c.auto |= c.fam == Fam::Uuid;
            }
        }
        view
    }

    /// The single uuid primary-key column a `create` fills with a fresh uuid when a row
    /// omits it.
    pub(crate) fn auto_uuid_pk(&self) -> Option<&str> {
        match self.pk().as_slice() {
            [pk] => self
                .col(pk)
                .filter(|c| c.fam == Fam::Uuid && !c.has_default)
                .map(|c| c.name.as_str()),
            _ => None,
        }
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
        if self.snap.no_id {
            Vec::new()
        } else if self.snap.pk.is_empty() {
            vec!["id".to_string()]
        } else {
            self.snap.pk.clone()
        }
    }

    /// Every unique key: the primary key, unique columns, and unique indexes.
    pub(crate) fn unique_keys(&self) -> Vec<Vec<String>> {
        let mut keys = Vec::new();
        let pk = self.pk();
        if !pk.is_empty() {
            keys.push(pk);
        }
        for c in self.snap.columns.iter().filter(|c| c.unique) {
            keys.push(vec![c.name.clone()]);
        }
        for i in self
            .snap
            .indexes
            .iter()
            .filter(|i| i.unique && i.raw.is_none())
        {
            keys.push(i.columns.clone());
        }
        keys
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
