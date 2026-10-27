//! The text sent on a command's stdin: the instruction, the masked statement, a slice of the schema and the tier.
use super::answer::{self, AnswerError};
use super::mask::{Literals, masked};
use super::slice::slice;
use crate::connection::Tier;
use crate::db::lex::{Lexed, Span, Token};
use crate::db::model::{Catalog, Statement};
use crate::history;
use std::fmt;

/// The most bytes a prompt holds, so a big schema never makes a slow or costly ask.
const MAX_BYTES: usize = 16 * 1024;
const CUT_NOTE: &str = "(The schema was cut to fit: more tables exist.)\n";

/// What the user asked for, sent as typed: whatever it holds leaves the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Instruction(String);

/// What one ask is built from.
#[derive(Debug)]
pub(crate) struct Ask<'a> {
    pub(crate) instruction: &'a Instruction,
    /// The statement under the cursor, none when the editor holds none there.
    pub(crate) statement: Option<&'a Statement>,
    pub(crate) catalog: &'a Catalog,
    pub(crate) tier: Tier,
}

/// The text for the command's stdin, and the literals it masked, to put back in the answer.
pub(crate) struct Prompt {
    text: String,
    literals: Literals,
}

/// Why an ask is not sent.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NotSent {
    /// The statement holds `PASSWORD` or a URL with a password, by the rule the history keeps.
    HoldsPassword,
    /// The statement holds a `{{` that opens no parameter, which no marker masks.
    UnreadableParam,
    /// The statement opens a quoted name or a string it never closes, which no marker masks whole.
    UnclosedQuote,
    /// The instruction and the statement alone pass the cap.
    TooLong,
}

impl Instruction {
    pub(crate) fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }
}

/// The prompt for `ask`, its schema slice cut table by table to stay within the cap, or why it is not sent.
pub(crate) fn prompt(ask: &Ask<'_>) -> Result<Prompt, NotSent> {
    let sql = ask.statement.map_or("", Statement::as_str);
    refuse_unsafe(sql)?;
    let masked = masked(sql);
    let head = head(ask, &masked.text);
    let room = MAX_BYTES.checked_sub(head.len() + CUT_NOTE.len()).ok_or(NotSent::TooLong)?;
    let schema = fitted(slice(ask.catalog, sql, &ask.instruction.0), room);
    Ok(Prompt { text: head + &schema, literals: masked.literals })
}

fn refuse_unsafe(sql: &str) -> Result<(), NotSent> {
    let lexed = Lexed::of(sql);
    let is_unreadable = lexed.tokens().iter().any(|placed| placed.token == Token::Unreadable);
    let is_unclosed = matches!(lexed.span_at(sql.len()), Span::String | Span::QuotedName);
    match () {
        () if history::holds_password(sql) => Err(NotSent::HoldsPassword),
        () if is_unreadable => Err(NotSent::UnreadableParam),
        () if is_unclosed => Err(NotSent::UnclosedQuote),
        () => Ok(()),
    }
}

fn head(ask: &Ask<'_>, statement: &str) -> String {
    let statement = if statement.is_empty() { "(none)" } else { statement };
    format!(
        "You write PostgreSQL for pgbg, a SQL editor. Answer with SQL statements only, in one ```sql fence.\n\
         The statement's values are masked: keep each '<literal n>' and <number n> where its value stays, and pgbg puts \
         the values back. Never write a marker the statement does not hold.\n\
         Keep each {{{{name}}}} parameter as it is.\n\
         The database is a {} one.\n\n\
         Instruction:\n{}\n\n\
         Statement under the cursor:\n{statement}\n\n\
         Schema, names and types:\n",
        ask.tier, ask.instruction.0
    )
}

/// The first `lines` that fit in `room` bytes, one a line, and a note when some were left out.
fn fitted(lines: Vec<String>, room: usize) -> String {
    let mut text = String::new();
    for line in lines {
        if text.len() + line.len() + 1 > room {
            return text + CUT_NOTE;
        }
        text += &line;
        text.push('\n');
    }
    text
}

impl Prompt {
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// The statements of the command's `reply`, with the literals this prompt masked put back.
    pub(crate) fn answer(&self, reply: &str) -> Result<Vec<Statement>, AnswerError> {
        answer::read(reply, &self.literals)
    }
}

/// The prompt holds the instruction and the literals hold the user's values: `Debug` says only how big they are.
impl fmt::Debug for Prompt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Prompt").field("bytes", &self.text.len()).field("literals", &self.literals).finish()
    }
}

impl fmt::Display for NotSent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::HoldsPassword => "the statement holds a password, so it is never sent",
            Self::UnreadableParam => "the statement holds a `{{` that opens no parameter: close it as `{{name}}` first",
            Self::UnclosedQuote => "the statement opens a quote it never closes: close it first",
            Self::TooLong => "the instruction and the statement pass 16 KiB: shorten one of them",
        })
    }
}

impl std::error::Error for NotSent {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model::{SchemaNode, Table, TableKind};
    use crate::fixtures::shop_catalog;

    fn prompt_for(statement: &str, instruction: &str, catalog: &Catalog) -> Result<Prompt, NotSent> {
        let instruction = Instruction::new(instruction);
        let statement = Statement::new(statement);
        prompt(&Ask { instruction: &instruction, statement: Some(&statement), catalog, tier: Tier::Production })
    }

    fn text(statement: &str, instruction: &str) -> String {
        prompt_for(statement, instruction, &shop_catalog()).unwrap().text().to_owned()
    }

    #[test]
    fn the_prompt_holds_the_instruction_the_masked_statement_the_slice_and_the_tier() {
        let text = text("SELECT * FROM orders WHERE total > 10 -- big ones", "only the ids");
        assert!(text.contains("Instruction:\nonly the ids\n"));
        assert!(text.contains("Statement under the cursor:\nSELECT * FROM orders WHERE total > <number 1>\n"));
        assert!(text.contains("public.orders(id integer, customer_id integer, total numeric(10,2))\n"));
        assert!(text.contains("a production one"));
    }

    #[test]
    fn no_literal_or_comment_of_the_statement_reaches_the_prompt() {
        let sql = "SELECT 'nina-s', E'nina-e\\'', $q$nina-d$q$, 4217 /* nina-c /* nina-n */ */ -- nina-l\nFROM orders";
        assert!(!text(sql, "fix it").contains("nina") && !text(sql, "fix it").contains("4217"));
    }

    #[test]
    fn no_default_estimate_or_index_of_the_schema_reaches_the_prompt() {
        let mut catalog = shop_catalog();
        catalog.schema.schemas[1].tables[0].columns[2].default = Some("'nina-default'".into());
        let text = prompt_for("SELECT * FROM orders", "", &catalog).unwrap().text().to_owned();
        assert!(!text.contains("nina-default") && !text.contains("1200") && !text.contains("orders_pkey"));
    }

    #[test]
    fn a_param_reaches_the_prompt_by_its_name() {
        assert!(text("SELECT * FROM orders WHERE id = {{order_id}}", "").contains("WHERE id = {{order_id}}"));
    }

    #[test]
    fn a_statement_holding_a_password_is_never_sent() {
        assert_eq!(prompt_for("ALTER ROLE nina PASSWORD 'x'", "", &shop_catalog()).unwrap_err(), NotSent::HoldsPassword);
    }

    #[test]
    fn a_statement_quoting_a_url_with_a_password_is_never_sent() {
        let sql = "SELECT dblink('postgresql://nina:x@db.example.com/shop', 'SELECT 1')";
        assert_eq!(prompt_for(sql, "", &shop_catalog()).unwrap_err(), NotSent::HoldsPassword);
    }

    #[test]
    fn a_statement_with_an_unreadable_param_is_never_sent() {
        assert_eq!(prompt_for("SELECT {{ 'nina'", "", &shop_catalog()).unwrap_err(), NotSent::UnreadableParam);
    }

    #[test]
    fn a_statement_with_a_quote_it_never_closes_is_never_sent() {
        for opener in ["\"total", "'nina", "E'nina", "$tag$ nina"] {
            let sql = format!("SELECT {opener} FROM orders WHERE note = 'nina' -- nina");
            assert_eq!(prompt_for(&sql, "", &shop_catalog()).unwrap_err(), NotSent::UnclosedQuote, "{opener}");
        }
    }

    #[test]
    fn a_comment_never_closed_never_reaches_the_prompt() {
        assert!(!text("SELECT id FROM orders /* nina 'x'", "").contains("nina"));
    }

    fn wide_catalog() -> Catalog {
        let mut catalog = shop_catalog();
        let columns = catalog.schema.schemas[1].tables[0].columns.clone();
        let table = |n: usize| Table {
            name: format!("orders_{n}"),
            kind: TableKind::Table,
            estimated_rows: None,
            columns: columns.clone(),
            indexes: vec![],
        };
        catalog.schema.schemas.push(SchemaNode { name: "archive".into(), tables: (0..400).map(table).collect() });
        catalog
    }

    #[test]
    fn a_slice_past_the_cap_is_cut_table_by_table_and_says_so() {
        let text = prompt_for("SELECT * FROM orders", "orders", &wide_catalog()).unwrap().text().to_owned();
        assert!(text.len() <= MAX_BYTES && text.ends_with(CUT_NOTE));
        assert!(text.contains("public.orders(id integer, customer_id integer, total numeric(10,2))\n"));
    }

    #[test]
    fn an_instruction_past_the_cap_is_refused() {
        assert_eq!(prompt_for("SELECT 1", &"x".repeat(MAX_BYTES), &shop_catalog()).unwrap_err(), NotSent::TooLong);
    }

    #[test]
    fn the_answer_gets_back_the_literals_this_prompt_masked() {
        let prompt = prompt_for("SELECT * FROM customers WHERE name = 'nina'", "", &shop_catalog()).unwrap();
        let statements = prompt.answer("```sql\nSELECT id FROM customers WHERE name = '<literal 1>';\n```").unwrap();
        assert_eq!(statements, [Statement::new("SELECT id FROM customers WHERE name = 'nina'")]);
    }

    #[test]
    fn debug_never_shows_the_prompt() {
        let prompt = prompt_for("SELECT 'nina'", "for nina", &shop_catalog()).unwrap();
        assert!(!format!("{prompt:?}").contains("nina"));
    }
}
