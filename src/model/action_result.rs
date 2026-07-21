use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub message: String,
}

impl Default for ActionResult {
    fn default() -> Self {
        Self {
            success: false,
            stdout: String::new(),
            stderr: String::new(),
            exit_code: -1,
            message: String::new(),
        }
    }
}
