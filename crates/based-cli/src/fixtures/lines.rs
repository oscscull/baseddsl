use crate::error::CliError;
use based_runtime::Engine;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Answer each stdin request line with one stdout line until stdin closes.
pub(super) async fn serve(engine: Engine) -> Result<(), CliError> {
    let mut requests = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    while let Some(line) = requests
        .next_line()
        .await
        .map_err(|e| CliError::io("reading stdin", e))?
    {
        if line.trim().is_empty() {
            continue;
        }
        let mut reply = answer(&engine, &line).await.to_string();
        reply.push('\n');
        stdout
            .write_all(reply.as_bytes())
            .await
            .map_err(|e| CliError::io("writing stdout", e))?;
        stdout
            .flush()
            .await
            .map_err(|e| CliError::io("writing stdout", e))?;
    }
    Ok(())
}

async fn answer(engine: &Engine, line: &str) -> Value {
    let request: Value = match serde_json::from_str(line) {
        Ok(request) => request,
        Err(e) => return error(&format!("each line must be one JSON object: {e}")),
    };
    let Some(model) = request["model"].as_str() else {
        return error("each request needs a \"model\" string");
    };
    let response = engine.insert_fixture(model, request["row"].clone()).await;
    if response.status == 200 {
        json!({ "row": response.body })
    } else {
        response.body
    }
}

fn error(message: &str) -> Value {
    json!({ "error": { "code": "bad_request", "message": message } })
}
