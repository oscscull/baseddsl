use super::*;

// ---------- upsert (`create … on conflict update`) live proof --------------

async fn ddl_backend(c: &Compiled) -> SqliteBackend {
    let backend = SqliteBackend::in_memory().expect("open in-memory sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("generated SQLite DDL failed: {e:?}\n{ddl}"));
    backend
}

#[tokio::test]
async fn upsert_inserts_then_composes_on_conflict() {
    // A page-view counter: the first hit inserts hits = 1, every later hit conflicts on the
    // unique `path` and increments the *stored* value server-side (the `hits = hits + 1`
    // atomic arithmetic in the conflict branch). The winning row reads back on both paths.
    let c = compile_sqlite(
        r#"
        Page { id: Id, path: text (unique), hits: int }
        shape PageRow from Page { path, hits }
        mutation record_hit(path: text) -> PageRow {
          create Page { path = $path, hits = 1 } on conflict (path) update { hits = hits + 1 };
        }
        "#,
    );
    let backend = ddl_backend(&c).await;
    // One shared id generator across requests — the real deployment holds a single engine
    // id gen (a fresh one per call would re-mint the same id and collide on `page.id`).
    let ids = SeqIdGen::default();
    let hit = |path: &'static str| {
        let c = &c;
        let backend = &backend;
        let ids = &ids;
        async move {
            dispatch(
                c,
                backend,
                "",
                ids,
                &NoStore,
                &Guards::new(),
                None,
                "POST",
                "/m/record_hit",
                json!({ "path": path }),
                json!({}),
                None,
            )
            .await
        }
    };

    // Insert path.
    let first = hit("/home").await;
    assert_eq!(first.status, 200, "{:?}", first.body);
    assert_eq!(first.body, json!({ "path": "/home", "hits": 1 }));

    // Conflict path — composes on the stored value, read-your-writes.
    assert_eq!(
        hit("/home").await.body,
        json!({ "path": "/home", "hits": 2 })
    );
    assert_eq!(
        hit("/home").await.body,
        json!({ "path": "/home", "hits": 3 })
    );

    // A different key is an independent insert.
    assert_eq!(
        hit("/about").await.body,
        json!({ "path": "/about", "hits": 1 })
    );
    // The first counter is untouched by the second key.
    assert_eq!(
        hit("/home").await.body,
        json!({ "path": "/home", "hits": 4 })
    );
}

/// An opaque `raw(…)` column and an opaque `@index raw(…)` execute against a real
/// database: the generated DDL creates both, a `create` writes the rest of the model
/// while the engine leaves the opaque column alone, and the declared shape reads it back
/// — bare (as an opaque string) and through the `raw` value leaf (a SQL function over it,
/// the only way to compute on a type the engine does not model).
#[tokio::test]
async fn opaque_column_and_index_execute_live() {
    let c = compile_sqlite(
        r#"
        Place {
          id:      Id
          name:    text
          shape_:  raw("blob")? (column "shape")
          @index raw("(lower(name))")
        }
        shape PlaceRow from Place {
          id
          name
          shape_
          shape_len = raw`length(shape)`
        }
        query place(id) -> PlaceRow;
        mutation add_place(name) -> PlaceRow {
          create Place { name = $name }
        }
        "#,
    );
    let backend = SqliteBackend::in_memory().expect("open sqlite");
    let ddl = sql::ddl(&c.schema, Dialect::Sqlite);
    // The opaque column's declared type and the opaque index's body ride into the DDL
    // verbatim — SQLite executes both.
    assert!(ddl.contains("`shape` blob NULL"), "\n{ddl}");
    assert!(ddl.contains("ON `place` (lower(name));"), "\n{ddl}");
    backend
        .execute_batch(&ddl)
        .await
        .unwrap_or_else(|e| panic!("DDL failed: {e:?}\n{ddl}"));

    // A create writes every modelled column and simply omits the opaque one.
    let created = call(
        &c,
        &backend,
        "POST",
        "/m/add_place",
        json!({ "name": "Dock" }),
        json!({}),
    )
    .await;
    assert_eq!(created.status, 200, "{:?}", created.body);
    assert_eq!(created.body["name"], json!("Dock"));
    assert_eq!(created.body["shape_"], json!(null));
    assert_eq!(created.body["shape_len"], json!(null));
    let id = created.body["id"]
        .as_str()
        .expect("generated id")
        .to_string();

    // A value the engine cannot construct still round-trips once the DB holds one: the
    // bare projection hands it back opaque, the raw leaf computes over it.
    backend
        .execute_batch(&format!(
            "UPDATE `place` SET `shape` = 'POINT(1 2)' WHERE `id` = '{id}';"
        ))
        .await
        .expect("set the opaque column out of band");
    let read = call(
        &c,
        &backend,
        "POST",
        "/q/place",
        json!({ "id": id }),
        json!({}),
    )
    .await;
    assert_eq!(read.status, 200, "{:?}", read.body);
    assert_eq!(read.body["name"], json!("Dock"));
    assert_eq!(read.body["shape_"], json!("POINT(1 2)"));
    assert_eq!(read.body["shape_len"], json!(10));
}
