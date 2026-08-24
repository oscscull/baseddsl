//! Aggregated core suite: the ungated runtime tests (dispatch, mutation/query planning,
//! the embedded client, load) in one binary. `embed` carries its own item-level `sqlite`
//! cfgs, so this stays ungated at the binary level.

#[path = "core/embed.rs"]
mod embed;
#[path = "core/load.rs"]
mod load;
#[path = "core/mutation.rs"]
mod mutation;
#[path = "core/query.rs"]
mod query;
#[path = "core/serve.rs"]
mod serve;
