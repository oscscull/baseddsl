//! Deterministic ASCII BSL spellings; physical identifiers are never changed.
pub(crate) fn words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let characters: Vec<_> = name.chars().collect();
    for (position, &character) in characters.iter().enumerate() {
        if character.is_ascii_alphanumeric() {
            let previous = position
                .checked_sub(1)
                .and_then(|position| characters.get(position));
            let next = characters.get(position + 1);
            if !word.is_empty()
                && character.is_ascii_uppercase()
                && (previous.is_some_and(|previous| {
                    previous.is_ascii_lowercase() || previous.is_ascii_digit()
                }) || (previous.is_some_and(char::is_ascii_uppercase)
                    && next.is_some_and(char::is_ascii_lowercase)))
            {
                words.push(std::mem::take(&mut word).to_ascii_lowercase());
            }
            word.push(character);
            continue;
        }
        if !word.is_empty() {
            words.push(std::mem::take(&mut word).to_ascii_lowercase());
        }
        if !character.is_ascii() {
            words.push(format!("u{:x}", u32::from(character)));
        }
    }
    if !word.is_empty() {
        words.push(word.to_ascii_lowercase());
    }
    words
}

pub(crate) fn model(name: &str) -> String {
    let mut name: String = words(name)
        .into_iter()
        .map(|mut word| {
            word[..1].make_ascii_uppercase();
            word
        })
        .collect();
    if name.is_empty()
        || name.starts_with(|character: char| character.is_ascii_digit())
        || reserved(&name)
    {
        name.insert_str(0, "Imported");
    }
    name
}

pub(crate) fn field(name: &str) -> String {
    let mut name = words(name).join("_");
    if name.is_empty()
        || name.starts_with(|character: char| character.is_ascii_digit())
        || reserved(&name)
    {
        name.insert_str(0, "imported_");
    }
    name
}

fn reserved(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "id" | "text"
            | "int"
            | "float"
            | "decimal"
            | "bool"
            | "timestamp"
            | "date"
            | "time"
            | "json"
            | "bytes"
            | "uuid"
            | "ulid"
            | "serial"
            | "query"
            | "mutation"
            | "shape"
            | "scope"
            | "enum"
            | "filter"
            | "from"
            | "where"
            | "list"
            | "get"
            | "create"
            | "update"
            | "delete"
            | "true"
            | "false"
            | "null"
            | "raw"
            | "default"
            | "unique"
            | "column"
    )
}

pub(crate) fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
