use super::*;

const CARDS: &str = r#"
    Card { id: Id, name: text, body: text, @index name }
    shape C from Card { name }
    filter excludes(banned) = not (body ~ any $banned);
    query q(terms: text[], banned: text[]) -> C[] {
      list Card where (body ~ all $terms and excludes($banned)) order (name);
    }
    query opt(terms?: text[]) -> C[] { list Card where (body ~ all $terms) order (name); }
    query bare(terms: text[] ~ any body) -> C[];
"#;

fn q(dialect: Dialect) -> String {
    query_section(&gen_for(CARDS, dialect), "q").to_string()
}

#[test]
fn sqlite_matches_against_json_each_rows() {
    let sql = q(Dialect::Sqlite);
    assert!(
        sql.contains(
            "NOT EXISTS (SELECT 1 FROM json_each(json_array(:terms)) AS __pattern \
             WHERE __pattern.value IS NOT NULL AND (`card`.`body` LIKE __pattern.value) IS NOT TRUE)"
        ),
        "\n{sql}"
    );
    assert!(
        sql.contains(
            "NOT (EXISTS (SELECT 1 FROM json_each(json_array(:banned)) AS __pattern \
             WHERE __pattern.value IS NOT NULL AND `card`.`body` LIKE __pattern.value))"
        ),
        "\n{sql}"
    );
}

#[test]
fn postgres_matches_against_an_unnested_text_array() {
    let sql = q(Dialect::Postgres);
    assert!(
        sql.contains(
            "NOT EXISTS (SELECT 1 FROM unnest(ARRAY[:terms]::text[]) AS __pattern(value) \
             WHERE __pattern.value IS NOT NULL AND (\"card\".\"body\" LIKE __pattern.value) IS NOT TRUE)"
        ),
        "\n{sql}"
    );
    assert!(
        sql.contains(
            "EXISTS (SELECT 1 FROM unnest(ARRAY[:banned]::text[]) AS __pattern(value) \
             WHERE __pattern.value IS NOT NULL AND \"card\".\"body\" LIKE __pattern.value)"
        ),
        "\n{sql}"
    );
}

#[test]
fn mysql_family_matches_against_json_table_rows() {
    for dialect in [Dialect::MariaDb, Dialect::MySql] {
        let sql = q(dialect);
        assert!(
            sql.contains(
                "NOT EXISTS (SELECT 1 FROM JSON_TABLE(JSON_ARRAY(:terms), '$[*]' \
                 COLUMNS (value JSON PATH '$')) AS __pattern WHERE JSON_TYPE(__pattern.value) = 'STRING' \
                 AND (`card`.`body` LIKE JSON_UNQUOTE(__pattern.value)) IS NOT TRUE)"
            ),
            "\n{sql}"
        );
    }
}

#[test]
fn a_signature_binding_lowers_like_the_predicate_form() {
    let sql = query_section(&gen_for(CARDS, Dialect::Sqlite), "bare").to_string();
    assert!(
        sql.contains(
            "EXISTS (SELECT 1 FROM json_each(json_array(:terms)) AS __pattern \
             WHERE __pattern.value IS NOT NULL AND `card`.`body` LIKE __pattern.value)"
        ),
        "\n{sql}"
    );
}

#[test]
fn an_optional_list_is_present_guarded() {
    let sql = query_section(&gen_for(CARDS, Dialect::Sqlite), "opt").to_string();
    assert!(
        sql.contains(
            "(:terms__present = 0 OR NOT EXISTS (SELECT 1 FROM json_each(json_array(:terms))"
        ),
        "\n{sql}"
    );
}
