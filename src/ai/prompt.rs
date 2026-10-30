//! The text sent on a command's stdin: the instruction, the masked statement, a slice of the schema and the tier.
use super::answer::{self, AnswerError};
use super::mask::{LiteralPolicy, Literals, Masked, masked, masked_comment};
use super::slice::slice;
use crate::connection::Tier;
use crate::db::lex::{Lexed, Span, Token};
use crate::db::model::{Catalog, Statement};
use crate::secret;
use std::fmt;

/// The most bytes a prompt holds, so a big schema never makes a slow or costly ask.
const MAX_BYTES: usize = 16 * 1024;
const CUT_NOTE: &str = "(The schema was cut to fit: more tables exist.)\n";
const MASKING_NOTE: &str = "Values are masked: keep each '<literal n>' and <number n> where its value stays, and pgbg puts the values \
                            back. Never write a marker the instruction or the statement does not hold.\n";

/// What the user asked for: typed for the ask and sent as typed, whatever it holds leaving the machine; or read off the
/// `--` lines above the statement, then pgbg's own words, the comment masked as the statement is, since nobody sent it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Instruction {
    comment: Option<String>,
    text: String,
}

/// What one ask is built from.
#[derive(Debug)]
pub(crate) struct PromptInput<'a> {
    pub(crate) instruction: &'a Instruction,
    /// The statement under the cursor, none when the editor holds none there.
    pub(crate) statement: Option<&'a Statement>,
    pub(crate) catalog: &'a Catalog,
    pub(crate) tier: Tier,
    pub(crate) literals: LiteralPolicy,
}

/// The text for the command's stdin, and the literals it masked, to put back in the answer.
#[derive(PartialEq, Eq)]
pub(crate) struct Prompt {
    text: String,
    literals: Literals,
}

/// Why an ask is not sent.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NotSent {
    /// The statement holds a secret, by the rule the history keeps: `secret::holds_secret`.
    HoldsSecret,
    /// The statement holds a `{{` that opens no parameter, which no marker masks.
    UnreadableParam,
    /// The statement opens a quoted name or a string it never closes, which no marker masks whole.
    UnclosedQuote,
    /// The instruction and the statement alone pass the cap.
    TooLong,
    /// The comment read as the instruction opens a quote or a `{{` it never closes, so its values cannot be masked.
    UnmaskableComment,
}

impl Instruction {
    pub(crate) fn new(text: impl Into<String>) -> Self {
        Self { comment: None, text: text.into() }
    }

    /// `comment`, the `--` lines above the statement, followed by `text`.
    pub(crate) fn under_comment(comment: impl Into<String>, text: impl Into<String>) -> Self {
        Self { comment: Some(comment.into()), text: text.into() }
    }

    /// The instruction as typed, the comment first, for the schema slice to read the names it points to.
    fn whole(&self) -> String {
        self.comment.iter().chain([&self.text]).map(String::as_str).collect::<Vec<_>>().join("\n")
    }

    /// The instruction as the prompt shows it, its comment sent as typed or masked, as `policy` says, with markers
    /// counted on from the statement's `before` literals; the literals of both come back.
    fn masked(&self, before: Literals, policy: LiteralPolicy) -> Result<Masked, NotSent> {
        let Some(comment) = &self.comment else { return Ok(Masked { text: self.text.clone(), literals: before }) };
        refuse_unsafe_comment(comment, policy)?;
        let comment = match policy {
            LiteralPolicy::Masked => masked_comment(comment, before),
            LiteralPolicy::Sent => Masked { text: comment.clone(), literals: before },
        };
        Ok(Masked { text: format!("{}\n{}", comment.text, self.text), literals: comment.literals })
    }
}

/// The prompt for `input`, its schema slice cut table by table to stay within the cap, or why it is not sent.
pub(crate) fn prompt(input: &PromptInput<'_>) -> Result<Prompt, NotSent> {
    let sql = input.statement.map_or("", Statement::as_str);
    refuse_unsafe(sql)?;
    let statement = masked(sql, input.literals);
    let instruction = input.instruction.masked(statement.literals, input.literals)?;
    let head = head(input, &instruction.text, &statement.text);
    let room = MAX_BYTES.checked_sub(head.len() + CUT_NOTE.len()).ok_or(NotSent::TooLong)?;
    let schema = fitted(slice(input.catalog, sql, &input.instruction.whole()), room);
    Ok(Prompt { text: head + &schema, literals: instruction.literals })
}

/// A comment holding a secret is never sent; with literals masked, nor is one whose values no marker masks whole.
fn refuse_unsafe_comment(comment: &str, policy: LiteralPolicy) -> Result<(), NotSent> {
    match (refuse_unsafe(comment), policy) {
        (Err(NotSent::HoldsSecret), _) => Err(NotSent::HoldsSecret),
        (Err(_), LiteralPolicy::Masked) => Err(NotSent::UnmaskableComment),
        _ => Ok(()),
    }
}

fn refuse_unsafe(sql: &str) -> Result<(), NotSent> {
    let lexed = Lexed::of(sql);
    let is_unreadable = lexed.tokens().iter().any(|placed| placed.token == Token::Unreadable);
    let is_unclosed = matches!(lexed.span_at(sql.len()), Span::String | Span::QuotedName);
    match () {
        () if secret::holds_secret(sql) => Err(NotSent::HoldsSecret),
        () if is_unreadable => Err(NotSent::UnreadableParam),
        () if is_unclosed => Err(NotSent::UnclosedQuote),
        () => Ok(()),
    }
}

fn head(input: &PromptInput<'_>, instruction: &str, statement: &str) -> String {
    let statement = if statement.is_empty() { "(none)" } else { statement };
    let masking = match input.literals {
        LiteralPolicy::Masked => MASKING_NOTE,
        LiteralPolicy::Sent => "",
    };
    format!(
        "You write PostgreSQL for pgbg, a SQL editor. Answer with SQL statements only, in one ```sql fence.\n\
         {masking}\
         Keep each {{{{name}}}} parameter as it is.\n\
         The database is a {} one.\n\n\
         Instruction:\n{instruction}\n\n\
         Statement under the cursor:\n{statement}\n\n\
         Schema:\n",
        input.tier
    )
}

/// The `lines` that fit in `room` bytes, one a line, in order, each too long for the room left skipped, and a note when
/// some were.
fn fitted(lines: impl Iterator<Item = String>, room: usize) -> String {
    let mut text = String::new();
    let mut is_cut = false;
    for line in lines {
        if text.len() + line.len() + 1 > room {
            is_cut = true;
            continue;
        }
        text += &line;
        text.push('\n');
    }
    if is_cut { text + CUT_NOTE } else { text }
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
            Self::HoldsSecret => "the statement holds a secret, so it is never sent",
            Self::UnreadableParam => "the statement holds a `{{` that opens no parameter: close it as `{{name}}` first",
            Self::UnclosedQuote => "the statement opens a quote it never closes: close it first",
            Self::TooLong => "the instruction and the statement pass 16 KiB: shorten one of them",
            Self::UnmaskableComment => {
                "the comment above the statement opens a quote or a `{{` it never closes, so its values cannot be masked"
            }
        })
    }
}

impl std::error::Error for NotSent {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model::SchemaNode;
    use crate::fixtures::{schema_table, shop_catalog};

    fn prompt_with(statement: &str, instruction: &str, catalog: &Catalog, literals: LiteralPolicy) -> Result<Prompt, NotSent> {
        let instruction = Instruction::new(instruction);
        let statement = Statement::new(statement);
        prompt(&PromptInput { instruction: &instruction, statement: Some(&statement), catalog, tier: Tier::Production, literals })
    }

    fn prompt_for(statement: &str, instruction: &str, catalog: &Catalog) -> Result<Prompt, NotSent> {
        prompt_with(statement, instruction, catalog, LiteralPolicy::Masked)
    }

    fn text(statement: &str, instruction: &str) -> String {
        prompt_for(statement, instruction, &shop_catalog()).unwrap().text().to_owned()
    }

    #[test]
    fn the_prompt_holds_the_instruction_the_masked_statement_the_slice_and_the_tier() {
        let text = text("SELECT * FROM orders WHERE total > 10 -- big ones", "only the ids");
        assert!(text.contains("Instruction:\nonly the ids\n"));
        assert!(text.contains("Statement under the cursor:\nSELECT * FROM orders WHERE total > <number 1>\n"));
        assert!(text.contains("public.orders(id integer, customer_id integer references public.customers(id), total numeric(10,2))\n"));
        assert!(text.contains("a production one"));
    }

    #[test]
    fn no_literal_or_comment_of_the_statement_reaches_the_prompt() {
        let sql = "SELECT 'nina-s', E'nina-e\\'', $q$nina-d$q$, 4217 /* nina-c /* nina-n */ */ -- nina-l\nFROM orders";
        assert!(!text(sql, "fix it").contains("nina") && !text(sql, "fix it").contains("4217"));
    }

    #[test]
    fn literals_sent_reach_the_prompt_as_typed_without_the_masking_note() {
        let text = prompt_with("SELECT * FROM orders WHERE total > 10 -- big ones", "", &shop_catalog(), LiteralPolicy::Sent).unwrap();
        assert!(text.text().contains("Statement under the cursor:\nSELECT * FROM orders WHERE total > 10\n"));
        assert!(!text.text().contains("masked"));
    }

    #[test]
    fn a_statement_holding_a_password_is_never_sent_even_with_literals_sent() {
        let refused = prompt_with("ALTER ROLE nina PASSWORD 'x'", "", &shop_catalog(), LiteralPolicy::Sent);
        assert_eq!(refused.unwrap_err(), NotSent::HoldsSecret);
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
        assert_eq!(prompt_for("ALTER ROLE nina PASSWORD 'x'", "", &shop_catalog()).unwrap_err(), NotSent::HoldsSecret);
    }

    #[test]
    fn a_statement_quoting_a_url_with_a_password_is_never_sent() {
        let sql = "SELECT dblink('postgresql://nina:x@db.example.com/shop', 'SELECT 1')";
        assert_eq!(prompt_for(sql, "", &shop_catalog()).unwrap_err(), NotSent::HoldsSecret);
    }

    #[test]
    fn a_statement_naming_a_secret_connection_key_is_never_sent() {
        let sql = "SELECT dblink_connect('host=db.example.com sslpassword=x')";
        assert_eq!(prompt_for(sql, "", &shop_catalog()).unwrap_err(), NotSent::HoldsSecret);
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
        let table = |n: usize| schema_table(&format!("orders_{n}"), columns.clone());
        catalog.schema.schemas.push(SchemaNode { name: "archive".into(), tables: (0..400).map(table).collect() });
        catalog
    }

    #[test]
    fn a_slice_past_the_cap_is_cut_table_by_table_and_says_so() {
        let text = prompt_for("SELECT * FROM orders", "orders", &wide_catalog()).unwrap().text().to_owned();
        assert!(text.len() <= MAX_BYTES && text.ends_with(CUT_NOTE));
        assert!(text.contains("public.orders(id integer, customer_id integer references public.customers(id), total numeric(10,2))\n"));
    }

    #[test]
    fn a_line_too_long_for_the_room_is_skipped_and_the_lines_after_it_still_fit() {
        let lines = ["a".repeat(10), "b".repeat(100), "c".to_owned()].into_iter();
        assert_eq!(fitted(lines, 20), format!("aaaaaaaaaa\nc\n{CUT_NOTE}"));
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

    fn under_comment(comment: &str, literals: LiteralPolicy) -> Result<Prompt, NotSent> {
        let instruction = Instruction::under_comment(comment, "finish it");
        let statement = Statement::new("SELECT * FROM orders");
        prompt(&PromptInput {
            instruction: &instruction,
            statement: Some(&statement),
            catalog: &shop_catalog(),
            tier: Tier::Production,
            literals,
        })
    }

    #[test]
    fn a_comment_instruction_is_masked_before_the_words_of_pgbg() {
        let text = under_comment("orders of nina@example.com", LiteralPolicy::Masked).unwrap().text().to_owned();
        assert!(text.contains("Instruction:\norders of '<literal 1>'\nfinish it\n"), "{text}");
    }

    #[test]
    fn a_comment_instruction_with_literals_sent_is_sent_as_typed() {
        let text = under_comment("don't show nina -- yet", LiteralPolicy::Sent).unwrap().text().to_owned();
        assert!(text.contains("Instruction:\ndon't show nina -- yet\nfinish it\n"), "{text}");
    }

    #[test]
    fn a_comment_instruction_opening_a_quote_is_unmaskable() {
        assert_eq!(under_comment("don't show nina", LiteralPolicy::Masked).unwrap_err(), NotSent::UnmaskableComment);
    }

    #[test]
    fn a_comment_instruction_holding_a_secret_is_never_sent_even_with_literals_sent() {
        assert_eq!(under_comment("set the password of nina", LiteralPolicy::Sent).unwrap_err(), NotSent::HoldsSecret);
    }

    #[test]
    fn debug_never_shows_the_prompt() {
        let prompt = prompt_for("SELECT 'nina'", "for nina", &shop_catalog()).unwrap();
        assert!(!format!("{prompt:?}").contains("nina"));
    }
}
