use pyo3::prelude::*;
use pyo3::types::PyDict;
use std::time::Instant;

#[derive(Debug, Clone, serde::Serialize)]
pub struct ExecutionResult {
    pub stdout: String,
    pub result: Option<String>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

pub struct PythonRuntime {
    locals_snapshot: Option<Vec<(String, String)>>,
}

impl PythonRuntime {
    pub fn new() -> Self {
        Self {
            locals_snapshot: None,
        }
    }

    pub fn execute(&mut self, code: &str) -> ExecutionResult {
        let start = Instant::now();

        Python::with_gil(|py| {
            // Capture stdout
            let sys = py.import("sys").unwrap();
            let io = py.import("io").unwrap();
            let string_io = io.call_method0("StringIO").unwrap();
            sys.setattr("stdout", &string_io).unwrap();

            // Set up globals with numpy and torch-like basics
            let globals = PyDict::new(py);
            let _ = py.run(
                c"import numpy as np",
                Some(&globals),
                None,
            );

            // Restore previous session state if any
            if let Some(ref snapshot) = self.locals_snapshot {
                for (key, val_repr) in snapshot {
                    let restore_code = format!("{} = {}", key, val_repr);
                    let _ = py.run(
                        std::ffi::CString::new(restore_code).unwrap().as_c_str(),
                        Some(&globals),
                        None,
                    );
                }
            }

            // Execute user code
            let exec_result = py.run(
                std::ffi::CString::new(code).unwrap().as_c_str(),
                Some(&globals),
                None,
            );

            // Get stdout
            let stdout: String = string_io
                .call_method0("getvalue")
                .and_then(|v| v.extract())
                .unwrap_or_default();

            // Reset stdout
            let new_stdout = io.call_method0("StringIO").unwrap();
            let _ = sys.setattr("stdout", &new_stdout);

            let duration_ms = start.elapsed().as_millis() as u64;

            match exec_result {
                Ok(_) => ExecutionResult {
                    stdout,
                    result: None,
                    error: None,
                    duration_ms,
                },
                Err(e) => ExecutionResult {
                    stdout,
                    result: None,
                    error: Some(e.to_string()),
                    duration_ms,
                },
            }
        })
    }

    pub fn clear_state(&mut self) {
        self.locals_snapshot = None;
    }
}
