use super::*;

// ---------- nested to-one shape sub-objects (L1) ---------------------------

#[test]
fn nested_to_one_forward_projects_prefixed_columns() {
    // A to-one `placed_by { … }` nest projects the joined User's columns under a
    // `placed_by.`-prefixed alias the runtime reassembles into a sub-object. The join is
    // the same one a reach-rename would build (reused machinery).
    let ddl = gen(r#"
        @soft_delete(deleted_at)
        User { id: Id, deleted_at: timestamp?, name: text, email: text }
        @soft_delete(deleted_at)
        @sort(placed_at desc)
        Order { id: Id, deleted_at: timestamp?, placed_by: User, total: int, placed_at: timestamp }
        shape OrderCard from Order { total, placed_by { name, email } }
        query order_by_id(id) -> OrderCard;
        "#);
    // the local column stays flat, the nested columns carry the `placed_by.` prefix.
    assert!(ddl.contains("`order`.`total` AS `total`"), "\n{ddl}");
    assert!(
        ddl.contains("`j_placed_by`.`name` AS `placed_by.name`"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("`j_placed_by`.`email` AS `placed_by.email`"),
        "\n{ddl}"
    );
    // the relation join is materialized (required relation -> INNER JOIN).
    assert!(
        ddl.contains("JOIN `user` AS `j_placed_by` ON `j_placed_by`.`id` = `order`.`placed_by_id`"),
        "\n{ddl}"
    );
}

#[test]
fn optional_to_one_nest_projects_a_presence_probe() {
    // A LEFT-JOINed to-one nest (optional relation) can come back all-NULL when the
    // row is absent; the child's `id` is projected once more as `<field>.__present`
    // so the runtime can collapse the sub-object to JSON null. A required nest
    // inner-joins and needs no probe.
    let ddl = gen(r#"
        @soft_delete(deleted_at)
        User { id: Id, deleted_at: timestamp?, name: text, email: text }
        @soft_delete(deleted_at)
        @sort(placed_at desc)
        Order {
          id: Id
          deleted_at: timestamp?
          placed_by:  User
          courier:    User?
          total:      int
          placed_at:  timestamp
        }
        shape OrderCard from Order { total, placed_by { name }, courier { name, email } }
        query order_by_id(id) -> OrderCard;
        "#);
    assert!(
        ddl.contains("`j_courier`.`id` AS `courier.__present`"),
        "\n{ddl}"
    );
    assert!(!ddl.contains("`placed_by.__present`"), "\n{ddl}");
}

#[test]
fn optional_to_one_nest_inside_a_json_array_null_collapses() {
    // Inside a to-many JSON aggregate the sub-object is built in SQL, so the absent
    // row collapses there: CASE WHEN the child's id IS NULL THEN NULL.
    let ddl = gen(r#"
        @soft_delete(deleted_at)
        User { id: Id, deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        @sort(placed_at desc)
        Order {
          id: Id
          deleted_at: timestamp?
          total:      int
          placed_at:  timestamp
          items:      OrderItem[] (OrderItem.order)
        }
        @soft_delete(deleted_at)
        OrderItem { id: Id, deleted_at: timestamp?, order: Order, checker: User?, qty: int }
        shape OrderCard from Order { total, items { qty, checker { name } } }
        query order_by_id(id) -> OrderCard;
        "#);
    assert!(
        ddl.contains("CASE WHEN `j_checker`.`id` IS NULL THEN NULL ELSE"),
        "\n{ddl}"
    );
}

#[test]
fn nested_to_one_recurses_and_reaches_inside_nest() {
    // Nested-within-nested (`placed_by { org { name } }`) chains joins and deepens the
    // alias prefix (`placed_by.org.name`); a `=`-reach inside a nest resolves from the
    // nested model's alias.
    let ddl = gen(r#"
        Org { id: Id, name: text }
        @sort(id asc)
        User { id: Id, org: Org, name: text }
        @sort(id asc)
        Order { id: Id, placed_by: User, total: int }
        shape OrderCard from Order { total, placed_by { name, org { name }, org_name = org.name } }
        query order_by_id(id) -> OrderCard;
        "#);
    assert!(
        ddl.contains("`j_placed_by`.`name` AS `placed_by.name`"),
        "\n{ddl}"
    );
    // the doubly-nested column carries the full prefix chain.
    assert!(
        ddl.contains("`j_placed_by_org`.`name` AS `placed_by.org.name`"),
        "\n{ddl}"
    );
    // a `=`-reach inside the nest is prefixed by the nest it sits in.
    assert!(
        ddl.contains("`j_placed_by_org`.`name` AS `placed_by.org_name`"),
        "\n{ddl}"
    );
    // both joins exist, the second keyed off the first's alias.
    assert!(
        ddl.contains("JOIN `user` AS `j_placed_by` ON `j_placed_by`.`id` = `order`.`placed_by_id`"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains(
            "JOIN `org` AS `j_placed_by_org` ON `j_placed_by_org`.`id` = `j_placed_by`.`org_id`"
        ),
        "\n{ddl}"
    );
}

#[test]
fn nest_ref_lowers_identically_to_inline_nest() {
    // A named-shape nest (`placed_by -> UserRef`) is a pure body expansion: the
    // emitted SELECT is byte-identical to the same fields nested inline — for a
    // to-one edge and for a to-many edge (correlated JSON subquery) alike.
    let common = r#"
        @soft_delete(deleted_at)
        User { id: Id, deleted_at: timestamp?, name: text, email: text }
        @soft_delete(deleted_at)
        @sort(placed_at desc)
        Order { id: Id, deleted_at: timestamp?, placed_by: User, total: int, placed_at: timestamp,
                items: OrderItem[] }
        OrderItem { id: Id, order: Order, sku: text, qty: int }
    "#;
    let inline = gen(&format!(
        r#"{common}
        shape OrderDetail from Order {{ total, placed_by {{ name, email }}, items {{ sku, qty }} }}
        query order_detail(id) -> OrderDetail;
        "#
    ));
    let named = gen(&format!(
        r#"{common}
        shape UserRef from User {{ name, email }}
        shape ItemRow from OrderItem {{ sku, qty }}
        shape OrderDetail from Order {{ total, placed_by -> UserRef, items -> ItemRow }}
        query order_detail(id) -> OrderDetail;
        "#
    ));
    let section = |s: &str| query_section(s, "order_detail").to_string();
    assert_eq!(section(&inline), section(&named), "\n{named}");
}

#[test]
fn nest_ref_recurses_through_named_shapes() {
    // A referenced shape may itself reference: `placed_by -> UserRef` where UserRef
    // nests `org -> OrgRef` chains the joins and prefixes like inline nesting.
    let ddl = gen(r#"
        Org { id: Id, name: text }
        @sort(id asc)
        User { id: Id, org: Org, name: text }
        @sort(id asc)
        Order { id: Id, placed_by: User, total: int }
        shape OrgRef from Org { name }
        shape UserRef from User { name, org -> OrgRef }
        shape OrderDetail from Order { total, placed_by -> UserRef }
        query order_detail(id) -> OrderDetail;
        "#);
    assert!(
        ddl.contains("`j_placed_by_org`.`name` AS `placed_by.org.name`"),
        "\n{ddl}"
    );
}

// ---------- required joins beneath an optional one ---------------------------

const REPORTS: &str = r#"
    User { id: Id, name: text }
    Post { id: Id, title: text, author: User }
    @sort(id asc)
    Report { id: Id, message: text, post: Post? }
    shape UserRef from User { id, name }
    shape PostRef from Post { id, title, author -> UserRef }
    shape ReportRow from Report { id, message, post -> PostRef }
    query reports() -> ReportRow[] { list Report; }
    query reports_by_author(name: text) -> ReportRow[] {
      list Report where (post.author.name = $name);
    }
    "#;

const GROUPED_BACKTICK: &str = "LEFT JOIN (`post` AS `j_post`\nJOIN `user` AS `j_post_author` ON \
     `j_post_author`.`id` = `j_post`.`author_id`) ON `j_post`.`id` = `report`.`post_id`";

#[test]
fn required_join_under_optional_one_groups_into_the_left_join() {
    // An inner join against the optional join's NULL side would drop every report
    // without a post, so the author join renders inside the post join's group.
    let my = gen(REPORTS);
    let q = query_section(&my, "reports");
    assert!(q.contains(GROUPED_BACKTICK), "\n{q}");
    assert!(q.contains("`j_post`.`id` AS `post.__present`"), "\n{q}");
    assert!(!q.contains("`post.author.__present`"), "\n{q}");

    let lite = gen_for(REPORTS, Dialect::Sqlite);
    let q = query_section(&lite, "reports");
    assert!(q.contains(GROUPED_BACKTICK), "\n{q}");

    let pg = gen_pg(REPORTS);
    let q = query_section(&pg, "reports");
    assert!(
        q.contains(
            "LEFT JOIN (\"post\" AS \"j_post\"\nJOIN \"user\" AS \"j_post_author\" ON \
             \"j_post_author\".\"id\" = \"j_post\".\"author_id\") ON \
             \"j_post\".\"id\" = \"report\".\"post_id\""
        ),
        "\n{q}"
    );
}

#[test]
fn filter_through_optional_then_required_path_uses_the_grouped_join() {
    // The filter reads the grouped author column; a report without a post has a NULL
    // name there, which the comparison excludes.
    let ddl = gen(REPORTS);
    let q = query_section(&ddl, "reports_by_author");
    assert!(q.contains(GROUPED_BACKTICK), "\n{q}");
    assert!(q.contains("`j_post_author`.`name` = :name"), "\n{q}");
    assert_eq!(q.matches("AS `j_post_author`").count(), 1, "\n{q}");
}

#[test]
fn soft_deleted_required_join_under_optional_one_stays_in_the_group() {
    // A tombstoned author nulls the whole post branch and keeps the report.
    let ddl = gen(r#"
        @soft_delete(deleted_at)
        User { id: Id, deleted_at: timestamp?, name: text }
        @soft_delete(deleted_at)
        Post { id: Id, deleted_at: timestamp?, title: text, author: User }
        @sort(id asc)
        Report { id: Id, message: text, post: Post? }
        shape ReportRow from Report { id, post { title, author { name } } }
        query reports() -> ReportRow[] { list Report; }
        "#);
    assert!(
        ddl.contains(
            "LEFT JOIN (`post` AS `j_post`\nJOIN `user` AS `j_post_author` ON \
             `j_post_author`.`id` = `j_post`.`author_id` AND `j_post_author`.`deleted_at` IS NULL) \
             ON `j_post`.`id` = `report`.`post_id` AND `j_post`.`deleted_at` IS NULL"
        ),
        "\n{ddl}"
    );
}

#[test]
fn required_join_under_optional_one_groups_inside_a_json_array() {
    let ddl = gen(r#"
        User { id: Id, name: text, reports: Report[] (Report.filed_by) }
        Post { id: Id, title: text, author: User }
        @sort(id asc)
        Report { id: Id, filed_by: User, post: Post? }
        shape ReportRow from Report { id, post { title, author { name } } }
        shape UserReports from User { name, reports -> ReportRow }
        query user_reports(id) -> UserReports;
        "#);
    assert!(
        ddl.contains("LEFT JOIN (`post` AS `j_post`\nJOIN `user` AS `j_post_author` ON"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("CASE WHEN `j_post`.`id` IS NULL THEN NULL ELSE"),
        "\n{ddl}"
    );
}

#[test]
fn optional_join_under_an_optional_one_stays_ungrouped() {
    // A LEFT join beneath a LEFT join already keeps the outer row.
    let ddl = gen(r#"
        User { id: Id, name: text }
        Post { id: Id, title: text, editor: User? }
        @sort(id asc)
        Report { id: Id, post: Post? }
        shape ReportRow from Report { id, post { title, editor { name } } }
        query reports() -> ReportRow[] { list Report; }
        "#);
    assert!(
        ddl.contains("LEFT JOIN `post` AS `j_post` ON `j_post`.`id` = `report`.`post_id`"),
        "\n{ddl}"
    );
    assert!(
        ddl.contains("LEFT JOIN `user` AS `j_post_editor` ON"),
        "\n{ddl}"
    );
}
