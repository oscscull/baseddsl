/// Write one executed statement to stderr: its SQL on one line and how many values it binds.
pub(super) fn log_statement(sql: &str, binds: usize) {
    let sql = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    let unit = if binds == 1 { "bind" } else { "binds" };
    eprintln!("based sql: {sql} -- {binds} {unit}");
}
