use crate::embed::Engine;
use crate::idempotency::NoStore;
use crate::run::{run_mutation, run_mutation_on, DbRead};
use crate::serve::{run_result_to_response, WireResponse};

impl Engine {
    /// Insert one test-fixture row of `model` in its own transaction and return the whole
    /// row as stored. `row` maps field names to values (a relation to its target's key
    /// object); unset columns take NULL or their database default. Scopes and guards do
    /// not apply. Not reachable through any route.
    pub async fn insert_fixture(&self, model: &str, row: serde_json::Value) -> WireResponse {
        let prepared = match super::prepare(self.compiled(), model, row) {
            Ok(prepared) => prepared,
            Err(response) => return response,
        };
        let result = run_mutation(
            &prepared.compiled,
            self.backend(),
            "",
            self.id_gen(),
            &NoStore,
            &prepared.request,
        )
        .await;
        run_result_to_response(result)
    }

    /// Like [`Engine::insert_fixture`], on an open connection or transaction it does not
    /// commit.
    pub async fn insert_fixture_on<D: DbRead + ?Sized>(
        &self,
        db: &mut D,
        model: &str,
        row: serde_json::Value,
    ) -> WireResponse {
        let prepared = match super::prepare(self.compiled(), model, row) {
            Ok(prepared) => prepared,
            Err(response) => return response,
        };
        let result =
            run_mutation_on(&prepared.compiled, db, self.id_gen(), &prepared.request).await;
        run_result_to_response(result)
    }
}
