//! CPU-only bridge probes on representative inputs and already materialized results.
use crate::{
    case::{Case, Output},
    client, fixture,
    measure::{self, Summary},
    Error,
};
use serde::Serialize;

#[derive(Serialize)]
pub struct Conversion {
    pub input_json_bytes: usize,
    pub result_json_bytes: usize,
    pub input_to_value: Summary,
    pub result_from_value: Summary,
}

pub fn measure(case: Case, output: &Output, count: usize) -> Result<Conversion, Error> {
    let (_, input) = case.request();
    let result = match case {
        Case::Bulk(_) => serde_json::json!({}),
        _ => serde_json::to_value(output)?,
    };
    Ok(Conversion {
        input_json_bytes: serde_json::to_vec(&input)?.len(),
        result_json_bytes: serde_json::to_vec(&result)?.len(),
        input_to_value: input_probe(case, count),
        result_from_value: measure::cpu_prepared(
            count,
            || result.clone(),
            |value| decode(case, value).unwrap(),
        ),
    })
}

fn input_probe(case: Case, count: usize) -> Summary {
    match case {
        Case::Flat(size) => serialize_probe(&client::FlatInput { size }, count),
        Case::Nested(size) => serialize_probe(&client::NestedInput { size }, count),
        Case::Page => serialize_probe(&client::PagedInput { offset: Some(32) }, count),
        Case::Bulk(size) => serialize_probe(&fixture::bulk(size), count),
    }
}

fn serialize_probe(input: &impl Serialize, count: usize) -> Summary {
    measure::cpu(count, || serde_json::to_value(input).unwrap())
}

fn decode(case: Case, result: serde_json::Value) -> Result<Output, Error> {
    Ok(match case {
        Case::Flat(_) => Output::Items(serde_json::from_value(result)?),
        Case::Nested(_) => Output::Owners(serde_json::from_value(result)?),
        Case::Page => Output::Page(serde_json::from_value(result)?),
        Case::Bulk(_) => {
            let _: client::Ack = serde_json::from_value(result)?;
            Output::Ack(())
        }
    })
}
