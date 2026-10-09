//! Machine-readable benchmark evidence and measurement schema.
use crate::{conversion, measure, preflight};
use serde::Serialize;

#[derive(Serialize)]
pub struct ResultRow {
    pub case: String,
    pub statements: usize,
    pub evidence: Vec<preflight::Evidence>,
    pub direct_sqlx: measure::Summary,
    pub embedded: measure::Summary,
    pub conversion: conversion::Conversion,
}

#[derive(Serialize)]
pub struct Report {
    pub platform: String,
    pub sqlite_version: String,
    pub profile: String,
    pub connections: usize,
    pub startup_schema_load_us: f64,
    pub workloads: Vec<ResultRow>,
}
