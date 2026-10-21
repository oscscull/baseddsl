use super::*;

/// Bring-your-own transaction (`adopt`) against a real SQLite database (transactions.md rung
/// 3): a caller opens a transaction on its **own** connection, does a **raw non-baseddsl
/// write**, then runs baseddsl work (a `for update` read — a no-op lock on SQLite, but the
/// read still runs — and a mutation) through `AdoptedTransport` on that same transaction. The
/// raw write and the baseddsl write commit atomically, and roll back together when the caller
/// does not commit. `adopt` never begins/commits/rolls back — the caller owns the boundary.
#[tokio::test]
async fn adopt_commits_raw_and_baseddsl_writes_atomically_on_sqlite() {
    let c = compile_sqlite(
        r#"
        Widget { id: text, name: text, stock: int }
        shape WidgetRow from Widget { name, stock }
        query widget_for_update(id) -> WidgetRow {
            get Widget where (id = $id) for update;
        }
        mutation restock(id, stock) -> WidgetRow {
            update Widget where (id = $id) { stock = $stock }
        }
        "#,
    );
    crate::driver::adopt_commits_atomically(c).await;
}
