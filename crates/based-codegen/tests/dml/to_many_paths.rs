use super::*;

const POSTS: &str = r#"
    Org { id: Id, name: text, posts: Post[] (Post.org) }
    Post { id: Id, org: Org, title: text, comments: Comment[] (Comment.post) }
    @soft_delete(deleted_at)
    Comment { id: Id, post: Post, author: text, resolved: bool?, deleted_at: timestamp? }
    shape PostRow from Post { title }
    shape OrgRow from Org { name }
"#;

fn post_query(filter: &str) -> String {
    format!("{POSTS}query q() -> PostRow[] {{ list Post where ({filter}) order (title); }}")
}

#[test]
fn a_to_many_path_lowers_to_exists_on_every_dialect() {
    let src = post_query("comments.resolved = false");
    let maria = gen(&src);
    assert!(
        maria.contains(
            "EXISTS (SELECT 1 FROM `comment` AS `s1_comment` WHERE `s1_comment`.`post_id` = `post`.`id` AND `s1_comment`.`deleted_at` IS NULL AND `s1_comment`.`resolved` = "
        ),
        "\n{maria}"
    );
    // The child never joins into the outer SELECT, so a post can't fan out.
    assert!(!maria.contains("JOIN `comment`"), "\n{maria}");
    let lite = gen_for(&src, Dialect::Sqlite);
    assert!(
        lite.contains("EXISTS (SELECT 1 FROM `comment` AS `s1_comment` WHERE `s1_comment`.`post_id` = `post`.`id`"),
        "\n{lite}"
    );
    let pg = gen_pg(&src);
    assert!(
        pg.contains(r#"EXISTS (SELECT 1 FROM "comment" AS "s1_comment" WHERE "s1_comment"."post_id" = "post"."id""#),
        "\n{pg}"
    );
    assert!(!pg.contains(r#"JOIN "comment""#), "\n{pg}");
}

#[test]
fn not_over_a_to_many_path_means_no_related_row_matches() {
    let sql = gen(&post_query("not (comments.resolved = false)"));
    assert!(
        sql.contains("NOT (EXISTS (SELECT 1 FROM `comment` AS `s1_comment` WHERE `s1_comment`.`post_id` = `post`.`id`"),
        "\n{sql}"
    );
}

#[test]
fn a_null_test_through_a_to_many_path_needs_a_related_row() {
    let sql = gen(&post_query("comments.resolved = null"));
    assert!(
        sql.contains("AND `s1_comment`.`resolved` IS NULL)"),
        "\n{sql}"
    );
    assert!(!sql.contains("JOIN `comment`"), "\n{sql}");
}

#[test]
fn each_leaf_through_a_to_many_path_is_its_own_exists() {
    let sql = gen(&format!(
        "{POSTS}query q(a: text, b: text) -> PostRow[] {{ list Post where \
         (comments.resolved = false and comments.author in ($a, $b) and comments.resolved) \
         order (title); }}"
    ));
    assert!(sql.contains("`s1_comment`.`resolved` = "), "\n{sql}");
    assert!(sql.contains("`s2_comment`.`author` IN (:a, :b)"), "\n{sql}");
    assert!(sql.contains("`s3_comment`.`resolved` = "), "\n{sql}");
}

#[test]
fn a_column_operand_still_reads_the_outer_row() {
    let sql = gen(&post_query("comments.author = title"));
    assert!(
        sql.contains("`s1_comment`.`author` = `post`.`title`"),
        "\n{sql}"
    );
}

#[test]
fn a_multi_hop_to_many_path_nests_exists() {
    let src = format!(
        "{POSTS}query q() -> OrgRow[] {{ list Org where (posts.comments.resolved = false) order (name); }}"
    );
    let sql = gen(&src);
    assert!(
        sql.contains(
            "EXISTS (SELECT 1 FROM `post` AS `s1_post` WHERE `s1_post`.`org_id` = `org`.`id` AND EXISTS (SELECT 1 FROM `comment` AS `s2_comment` WHERE `s2_comment`.`post_id` = `s1_post`.`id`"
        ),
        "\n{sql}"
    );
}

#[test]
fn to_one_hops_before_a_to_many_hop_still_join() {
    let src = format!(
        "{POSTS}shape CommentRow from Comment {{ author }}\
         query q(t: text) -> CommentRow[] {{ list Comment where (post.org.posts.title = $t) order (author); }}"
    );
    let sql = gen(&src);
    assert!(sql.contains("JOIN `org` AS `j_post_org`"), "\n{sql}");
    assert!(
        sql.contains("EXISTS (SELECT 1 FROM `post` AS `s1_post` WHERE `s1_post`.`org_id` = `j_post_org`.`id` AND `s1_post`.`title` = :t)"),
        "\n{sql}"
    );
}
