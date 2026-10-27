//! The `[ai] command`, asked off the loop: one ask at a time, its reply read back against the prompt it was asked with.
use super::super::app::{Answer, AskFailure, AskId};
use super::send;
use crate::ai::{Asking, Prompt};
use crate::config::AiSettings;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

/// The ask whose command runs, and where its answer goes.
#[derive(Debug)]
pub(super) struct Asks {
    answers: UnboundedSender<Answer>,
    running: Option<Running>,
}

/// The task waiting on an ask's reply: aborted when dropped, which drops its `Asking` and kills the command's group.
#[derive(Debug)]
struct Running {
    ask: AskId,
    task: JoinHandle<()>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Asks {
    pub(super) fn new(answers: UnboundedSender<Answer>) -> Self {
        Self { answers, running: None }
    }

    /// Asks the command of `settings` with `prompt` and kills the one asked before; the answer comes back as
    /// `Answer::Asked`.
    pub(super) fn ask(&mut self, ask: AskId, prompt: Prompt, settings: &AiSettings) {
        let asking = Asking::start(settings, prompt.text());
        let answers = self.answers.clone();
        let task = tokio::spawn(async move {
            let answer = match asking.reply().await {
                Ok(reply) => prompt.answer(&reply).map_err(AskFailure::Answer),
                Err(failed) => Err(AskFailure::Command(failed)),
            };
            send(&answers, Answer::Asked { ask, answer });
        });
        self.running = Some(Running { ask, task });
    }

    /// Kills the command of `ask` if it is the one running.
    pub(super) fn drop_ask(&mut self, ask: AskId) {
        if self.running.as_ref().is_some_and(|running| running.ask == ask) {
            self.running = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{self, Instruction, LiteralPolicy};
    use crate::connection::Tier;
    use crate::db::model::{Catalog, Statement};
    use crate::fixtures::{is_gone, leaving_a_sleep, written_pid};
    use std::time::Duration;
    use tokio::sync::mpsc::{self, UnboundedReceiver};

    fn asks(command: &[String]) -> (Asks, AiSettings, UnboundedReceiver<Answer>) {
        let settings = AiSettings {
            command: command.to_vec(),
            env: vec!["PATH".to_owned()],
            timeout: Duration::from_secs(60),
            literals: LiteralPolicy::Masked,
        };
        let (sender, answers) = mpsc::unbounded_channel();
        (Asks::new(sender), settings, answers)
    }

    fn prompt(sql: &str) -> Prompt {
        let (instruction, statement) = (Instruction::new("only the ids"), Statement::new(sql));
        let catalog = Catalog::default();
        let ask = ai::Ask {
            instruction: &instruction,
            statement: Some(&statement),
            catalog: &catalog,
            tier: Tier::Local,
            literals: LiteralPolicy::Masked,
        };
        ai::prompt(&ask).unwrap()
    }

    #[tokio::test]
    #[ignore = "starts a process"]
    async fn the_reply_comes_back_as_statements_with_their_literals() {
        let reply = "cat > /dev/null; printf '```sql\\nSELECT id FROM orders WHERE customer = <literal 1>;\\n```\\n'";
        let (mut asks, settings, mut answers) = asks(&["sh".to_owned(), "-c".to_owned(), reply.to_owned()]);
        asks.ask(AskId::FIRST, prompt("SELECT * FROM orders WHERE customer = 'nina'"), &settings);
        let statements = vec![Statement::new("SELECT id FROM orders WHERE customer = 'nina'")];
        assert_eq!(answers.recv().await, Some(Answer::Asked { ask: AskId::FIRST, answer: Ok(statements) }));
    }

    #[tokio::test]
    #[ignore = "starts a process"]
    async fn a_dropped_ask_kills_its_command() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("sleep.pid");
        let (mut asks, settings, _answers) = asks(&leaving_a_sleep(&pid_file));
        asks.ask(AskId::FIRST, prompt("SELECT 1"), &settings);
        let sleep = written_pid(&pid_file).await;
        asks.drop_ask(AskId::FIRST);
        assert!(is_gone(sleep).await);
    }
}
