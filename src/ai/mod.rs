//! Asking a command for SQL, pure: the prompt sent on its stdin, and its answer read back as statements.
//! A prompt holds no literal, comment or password of the statement, and no value of the schema but names and types.
mod answer;
mod mask;
mod prompt;
mod slice;
