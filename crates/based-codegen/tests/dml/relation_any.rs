use super::*;

const CARDS: &str = r#"
    Card { id: Id, name: text, paper: bool, printings: Printing[] }
    @soft_delete(deleted_at)
    Printing { id: Id, card: Card, digital: bool, lang: text, deleted_at: timestamp? }
    shape CardRow from Card { name }
"#;

#[test]
fn any_lowers_to_a_correlated_exists_on_every_dialect() {
    let src = format!(
        "{CARDS}query paper() -> CardRow[] {{ list Card where (printings.any(digital = false)) order (name); }}"
    );
    let maria = gen(&src);
    assert!(
        maria.contains(
            "EXISTS (SELECT 1 FROM `printing` AS `s1_printing` WHERE `s1_printing`.`card_id` = `card`.`id` AND `s1_printing`.`deleted_at` IS NULL AND `s1_printing`.`digital` = "
        ),
        "\n{maria}"
    );
    let lite = gen_for(&src, Dialect::Sqlite);
    assert!(
        lite.contains("EXISTS (SELECT 1 FROM `printing` AS `s1_printing` WHERE `s1_printing`.`card_id` = `card`.`id`"),
        "\n{lite}"
    );
    let pg = gen_pg(&src);
    assert!(
        pg.contains(r#"EXISTS (SELECT 1 FROM "printing" AS "s1_printing" WHERE "s1_printing"."card_id" = "card"."id""#),
        "\n{pg}"
    );
    // The child rows never join into the outer SELECT (no row fan-out).
    assert!(!maria.contains("JOIN `printing`"), "\n{maria}");
}

#[test]
fn bare_any_and_negation_test_existence_only() {
    let src = format!(
        "{CARDS}query orphans() -> CardRow[] {{ list Card where (not printings.any()) order (name); }}"
    );
    let sql = gen(&src);
    assert!(
        sql.contains(
            "NOT (EXISTS (SELECT 1 FROM `printing` AS `s1_printing` WHERE `s1_printing`.`card_id` = `card`.`id` AND `s1_printing`.`deleted_at` IS NULL))"
        ),
        "\n{sql}"
    );
}

#[test]
fn any_binds_params_inside_the_subquery() {
    let src = format!(
        "{CARDS}query in_lang(lang: text) -> CardRow[] {{ list Card where (printings.any(lang = $lang and digital = false)) order (name); }}"
    );
    let sql = gen(&src);
    assert!(sql.contains("`s1_printing`.`lang` = :lang"), "\n{sql}");
}
