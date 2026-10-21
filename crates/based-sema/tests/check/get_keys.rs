use super::*;

// ---------- `get` keying (E0144) ------------------------------------------

const ITEM: &str = r#"
    Item {
      id:          Id
      title:       text
      source:      text
      external_id: text?
      @index(source, external_id) unique
    }
    shape ItemRow from Item { title }
"#;

fn check_item(queries: &str) -> Vec<String> {
    errors(&analyze(&format!("{ITEM}{queries}")).1)
        .into_iter()
        .map(ToString::to_string)
        .collect()
}

#[test]
fn get_keyed_on_composite_unique_index_params() {
    assert!(check_item("query item_by_external(source, external_id) -> ItemRow;").is_empty());
}

#[test]
fn get_keyed_on_composite_unique_index_block() {
    let q = r#"
        query item_by_external(source: text, external_id: text) -> ItemRow {
          get Item where (source = $source and external_id = $external_id);
        }
    "#;
    assert!(check_item(q).is_empty(), "{:?}", check_item(q));
}

#[test]
fn get_on_part_of_composite_unique_index_is_not_keyed() {
    assert_eq!(
        check_item("query item_by_source(source) -> ItemRow;"),
        ["E0144"]
    );
}

#[test]
fn get_with_unique_eq_under_or_is_not_keyed() {
    let q = r#"
        query item_or(id: Id, title: text) -> ItemRow {
          get Item where (id = $id or title = $title);
        }
    "#;
    assert_eq!(check_item(q), ["E0144"]);
}

#[test]
fn get_with_unique_eq_under_not_is_not_keyed() {
    let q = r#"
        query item_not(id: Id) -> ItemRow {
          get Item where (not (id = $id));
        }
    "#;
    assert_eq!(check_item(q), ["E0144"]);
}

#[test]
fn get_with_unique_eq_anded_with_or_is_keyed() {
    let q = r#"
        query item_and(id: Id, title: text) -> ItemRow {
          get Item where (id = $id and (title = $title or source = $title));
        }
    "#;
    assert!(check_item(q).is_empty(), "{:?}", check_item(q));
}
