//! Control commands answer the way herdr's do: JSON on stdout,
//! `{"id":"cli:<group>:<command>","result":{..}}`; on failure the same
//! envelope with `error` goes to stderr and the exit code is 1.

use std::path::PathBuf;
use std::process::ExitCode;

use crate::api::{self, ErrorResponse, PROTOCOL, SCHEMA_VERSION};

#[derive(Debug)]
pub struct CliError {
    pub code: String,
    pub message: String,
}

impl CliError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
        }
    }

    pub fn io(e: std::io::Error) -> Self {
        Self::new("io_error", e.to_string())
    }
}

/// Reports the outcome of `cli:<id>` and gives the exit code.
pub fn finish(id: &str, result: Result<(), CliError>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            let body = ErrorResponse::new(format!("cli:{id}"), &e.code, e.message);
            eprintln!("{}", serde_json::to_string(&body).unwrap_or_default());
            ExitCode::FAILURE
        }
    }
}

/// `herdfold api schema [--json | --output PATH]`
pub fn api_schema(json: bool, output: Option<PathBuf>) -> Result<(), CliError> {
    let schema = api::schema();
    if let Some(path) = output {
        let text = serde_json::to_string_pretty(&schema)
            .map_err(|e| CliError::new("internal", e.to_string()))?;
        std::fs::write(&path, text + "\n").map_err(CliError::io)?;
        println!("wrote API schema to {}", path.display());
    } else if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&schema).unwrap_or_default()
        );
    } else {
        let names: Vec<&String> = schema["schemas"]
            .as_object()
            .map(|m| m.keys().collect())
            .unwrap_or_default();
        let names: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        println!("Herdfold API schema");
        println!("protocol: {PROTOCOL}");
        println!("schema_version: {SCHEMA_VERSION}");
        println!("schemas: {}", names.join(", "));
        println!();
        println!("Use `herdfold api schema --json` to print the full schema.");
        println!("Use `herdfold api schema --output PATH` to write it to a file.");
    }
    Ok(())
}
