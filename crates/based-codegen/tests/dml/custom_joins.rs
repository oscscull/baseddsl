use super::*;

// ---------- custom `on:` join conditions --------------------------------------
// A relation with a custom `on:` join (legacy keys that don't follow `<field>_id`)
// must render its declared condition as the join `ON` — not the conventional
// `<field>_id = pk` correlation, which for a custom-join edge names a column that
// doesn't exist. Covers the forward to-one join and the to-many inverse over it.

const CUSTOM_JOIN_SCHEMA: &str = r#"
    Order { id: Id  user_ref: int  placed_by: User (on: order.user_ref = user.legacy_id) }
    User { id: Id, name: text, legacy_id: int, orders: Order[] (Order.placed_by) }
    shape OrderCard from Order { user_ref, placed_by { name } }
    query order_by_id(id) -> OrderCard;
    shape UserCard from User { name, orders { user_ref } }
    query user_by_id(id) -> UserCard;
"#;

#[test]
fn custom_join_renders_the_declared_on_condition_forward() {
    // Forward `placed_by { … }` joins User on the custom condition, on every dialect.
    let maria = gen(CUSTOM_JOIN_SCHEMA);
    assert!(
        maria.contains(
            "JOIN `user` AS `j_placed_by` ON `order`.`user_ref` = `j_placed_by`.`legacy_id`"
        ),
        "\n{maria}"
    );
    // No conventional (nonexistent) `placed_by_id` column leaks into the join.
    assert!(!maria.contains("placed_by_id"), "\n{maria}");

    let pg = gen_pg(CUSTOM_JOIN_SCHEMA);
    assert!(
        pg.contains(
            r#"JOIN "user" AS "j_placed_by" ON "order"."user_ref" = "j_placed_by"."legacy_id""#
        ),
        "\n{pg}"
    );

    let lite = gen_for(CUSTOM_JOIN_SCHEMA, Dialect::Sqlite);
    assert!(
        lite.contains(
            "JOIN `user` AS `j_placed_by` ON `order`.`user_ref` = `j_placed_by`.`legacy_id`"
        ),
        "\n{lite}"
    );
}

#[test]
fn custom_join_correlates_a_to_many_inverse_via_the_condition() {
    // The to-many inverse `orders { … }` correlates the child's custom FK column
    // (`user_ref`) to the parent's referenced column (`legacy_id`), not `placed_by_id`.
    let maria = gen(CUSTOM_JOIN_SCHEMA);
    assert!(
        maria.contains(
            "FROM `order` AS `s1_order` WHERE `s1_order`.`user_ref` = `user`.`legacy_id`"
        ),
        "\n{maria}"
    );
    assert!(!maria.contains("placed_by_id"), "\n{maria}");
}
