//! Asking a command for SQL: the prompt sent on its stdin and its answer read back as statements, pure, and the
//! command run on it (`command.rs`).
//! A prompt holds no comment or password of the statement, no literal unless `[ai] literals = "sent"`, and no value of
//! the schema but names, types, enum labels, foreign keys and comments; the history only orders the tables it shows.
mod answer;
mod command;
mod mask;
mod prompt;
mod slice;

pub(crate) use answer::AnswerError;
pub(crate) use command::{Asking, CommandError};
pub(crate) use mask::LiteralPolicy;
pub(crate) use prompt::{Instruction, NotSent, Prompt, PromptInput, prompt};
pub(crate) use slice::TableUse;
