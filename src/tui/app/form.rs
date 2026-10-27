//! The connection form, from the picker's `ctrl-a` and `ctrl-e`: each key of a connection on its own line, checked as
//! the command line checks its flags, then written by the backend; an edit writes only the keys that changed.
use super::{Action, App, ConfigChangeId, KeychainPassword, Overlay, SaveConnection, Saving};
use crate::act::{self, Act, Pane};
use crate::config::ConnectionChange;
use crate::connection::{Connection, ConnectionName, StatementTimeout, Target, Tier, Tls, TunnelSpec, WritePolicy};
use crate::secret::{KeychainItem, Secret, SecretSource};
use anyhow::{Context, anyhow};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use vtui::field::Field;

const CANNOT_CLEAR: &str = "pgbg cannot take a key out of the config yet: edit config.toml for that";
const TIERS: [Tier; 3] = [Tier::Local, Tier::Staging, Tier::Production];
const WRITE_POLICIES: [WritePolicy; 2] = [WritePolicy::ReadOnly, WritePolicy::Allowed];
const PASSWORD_FROM: [PasswordFrom; 4] = [PasswordFrom::Nowhere, PasswordFrom::Keychain, PasswordFrom::Env, PasswordFrom::Command];
const TLS_SETTINGS: [Option<Tls>; 3] = [None, Some(Tls::On), Some(Tls::Off)];

/// One line of the form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum Slot {
    Name,
    Tier,
    Url,
    PasswordFrom,
    PasswordWhere,
    Password,
    Tunnel,
    TunnelBefore,
    Writes,
    Tls,
    StatementTimeout,
}

/// Where the password comes from, as the form offers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum PasswordFrom {
    Nowhere,
    Keychain,
    Env,
    Command,
}

/// Which way a choice line turns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Turn {
    Forward,
    Back,
}

/// A line refused, and why.
type Refusal = (Slot, String);

/// The form while it is open: what each line holds, the line with the cursor, and what was refused.
#[derive(Debug, PartialEq)]
pub(in crate::tui) struct Form {
    pub(in crate::tui) saving: Saving,
    /// The connection as the config had it, when the form edits one.
    was: Option<Connection>,
    pub(in crate::tui) at: Slot,
    pub(in crate::tui) name: Field,
    pub(in crate::tui) tier: Tier,
    pub(in crate::tui) url: Field,
    pub(in crate::tui) password_from: PasswordFrom,
    /// The keychain item, the variable or the command, as `password_from` says.
    pub(in crate::tui) password_where: Field,
    /// Typed for the keychain item, which saving writes; nothing typed leaves the item as it is.
    pub(in crate::tui) password: Secret,
    pub(in crate::tui) tunnel: Field,
    /// The command the tunnel runs first, once.
    pub(in crate::tui) tunnel_before: Field,
    pub(in crate::tui) writes: WritePolicy,
    /// The `tls` key; none leaves it to the url's `sslmode`.
    pub(in crate::tui) tls: Option<Tls>,
    /// Empty for the tier's.
    pub(in crate::tui) statement_timeout: Field,
    /// Why a line is refused, each beside its line.
    errors: Vec<Refusal>,
    /// The save on its way, until its answer comes.
    sent: Option<ConfigChangeId>,
    /// Why the last save failed.
    pub(in crate::tui) failure: Option<String>,
}

impl Form {
    /// An empty form for a new connection, read only and local until said otherwise.
    pub(in crate::tui) fn adding() -> Self {
        Self {
            saving: Saving::Add,
            was: None,
            at: Slot::Name,
            name: Field::default(),
            tier: Tier::Local,
            url: Field::default(),
            password_from: PasswordFrom::Nowhere,
            password_where: Field::default(),
            password: Secret::default(),
            tunnel: Field::default(),
            tunnel_before: Field::default(),
            writes: WritePolicy::ReadOnly,
            tls: None,
            statement_timeout: Field::default(),
            errors: vec![],
            sent: None,
            failure: None,
        }
    }

    /// The form on `connection` as the config has it.
    pub(in crate::tui) fn editing(connection: &Connection) -> Self {
        let (password_from, password_where) = password_lines(connection.password());
        let tunnel = connection.tunnel();
        Self {
            saving: Saving::Edit,
            was: Some(connection.clone()),
            at: Slot::Tier,
            name: Field::new(connection.name().to_string()),
            tier: connection.tier(),
            url: Field::new(connection.target().to_string()),
            password_from,
            password_where: Field::new(password_where),
            tunnel: Field::new(tunnel.map(|tunnel| shell_words::join(tunnel.command())).unwrap_or_default()),
            tunnel_before: Field::new(tunnel.and_then(TunnelSpec::before).map(shell_words::join).unwrap_or_default()),
            writes: connection.writes(),
            tls: connection.tls_setting(),
            statement_timeout: Field::new(connection.statement_timeout().own().map(|own| own.to_string()).unwrap_or_default()),
            ..Self::adding()
        }
    }

    /// The lines shown, in order: the name only for a new connection, the password's only where it has one, what runs
    /// before the tunnel only once it has a command.
    pub(in crate::tui) fn slots(&self) -> Vec<Slot> {
        let is_new = self.saving == Saving::Add;
        let has_where = self.password_from != PasswordFrom::Nowhere;
        let has_password = self.password_from == PasswordFrom::Keychain;
        let has_tunnel = !self.tunnel.text().trim().is_empty();
        let shown = [(Slot::Name, is_new), (Slot::Tier, true), (Slot::Url, true), (Slot::PasswordFrom, true)];
        let password = [(Slot::PasswordWhere, has_where), (Slot::Password, has_password)];
        let rest = [(Slot::Tunnel, true), (Slot::TunnelBefore, has_tunnel), (Slot::Writes, true), (Slot::Tls, true)];
        let all = shown.into_iter().chain(password).chain(rest).chain([(Slot::StatementTimeout, true)]);
        all.filter(|&(_, is_shown)| is_shown).map(|(slot, _)| slot).collect()
    }

    /// The text of the line with the cursor, when it is a text line.
    pub(in crate::tui) fn field(&self) -> Option<&Field> {
        match self.at {
            Slot::Name => Some(&self.name),
            Slot::Url => Some(&self.url),
            Slot::PasswordWhere => Some(&self.password_where),
            Slot::Tunnel => Some(&self.tunnel),
            Slot::TunnelBefore => Some(&self.tunnel_before),
            Slot::StatementTimeout => Some(&self.statement_timeout),
            Slot::Tier | Slot::PasswordFrom | Slot::Password | Slot::Writes | Slot::Tls => None,
        }
    }

    pub(in crate::tui) fn is_saving(&self) -> bool {
        self.sent.is_some()
    }

    /// Whether `id` is the save this form sent and waits on.
    pub(in crate::tui) fn is_waiting_on(&self, id: ConfigChangeId) -> bool {
        self.sent == Some(id)
    }

    /// Says why the save failed; the form stays as it was typed, with the password it sent back in it unless another
    /// was typed during the save.
    pub(in crate::tui) fn failed(&mut self, why: String, password: Option<Secret>) {
        self.sent = None;
        self.failure = Some(why);
        let is_password_empty = self.password.typed_count() == 0;
        if let Some(password) = password.filter(|_| is_password_empty) {
            self.password = password;
        }
    }

    pub(in crate::tui) fn error(&self, slot: Slot) -> Option<&str> {
        self.errors.iter().find(|(refused, _)| *refused == slot).map(|(_, why)| why.as_str())
    }

    /// Types `text` on the line with the cursor as vtui cleans a paste, on one line, but on the password line, which takes
    /// every character a password may hold but a line end; a choice line takes none.
    pub(super) fn paste(&mut self, text: &str) {
        let is_line_end = |c: char| matches!(c, '\r' | '\n');
        if self.at == Slot::Password {
            return text.chars().filter(|&c| !is_line_end(c)).for_each(|c| self.password.push(c));
        }
        if let Some(field) = self.field_mut() {
            field.paste(&text.replace(is_line_end, ""));
        }
    }

    fn field_mut(&mut self) -> Option<&mut Field> {
        match self.at {
            Slot::Name => Some(&mut self.name),
            Slot::Url => Some(&mut self.url),
            Slot::PasswordWhere => Some(&mut self.password_where),
            Slot::Tunnel => Some(&mut self.tunnel),
            Slot::TunnelBefore => Some(&mut self.tunnel_before),
            Slot::StatementTimeout => Some(&mut self.statement_timeout),
            Slot::Tier | Slot::PasswordFrom | Slot::Password | Slot::Writes | Slot::Tls => None,
        }
    }

    fn step(&mut self, by: isize) {
        let slots = self.slots();
        let at = slots.iter().position(|&slot| slot == self.at).unwrap_or_default();
        self.at = slots[at.saturating_add_signed(by).min(slots.len() - 1)];
    }

    /// `key` on the line with the cursor: a choice turns with `←` `→` or space, a text line edits.
    fn type_key(&mut self, key: KeyEvent) {
        match (turn_of(key), self.at) {
            (Some(turn), Slot::Tier) => self.tier = turned(&TIERS, self.tier, turn),
            (Some(turn), Slot::Writes) => self.writes = turned(&WRITE_POLICIES, self.writes, turn),
            (Some(turn), Slot::PasswordFrom) => self.password_from = turned(&PASSWORD_FROM, self.password_from, turn),
            (Some(turn), Slot::Tls) => self.tls = turned(&TLS_SETTINGS, self.tls, turn),
            _ => self.edit_line(key),
        }
    }

    fn edit_line(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char(c) if is_typed(key) => self.type_char(c),
            KeyCode::Backspace if self.at == Slot::Password => self.password.pop(),
            _ => self.move_in_line(key),
        }
    }

    fn type_char(&mut self, c: char) {
        if self.at == Slot::Password {
            return self.password.push(c);
        }
        if let Some(field) = self.field_mut() {
            field.insert(c);
        }
    }

    fn move_in_line(&mut self, key: KeyEvent) {
        let Some(field) = self.field_mut() else { return };
        match key.code {
            KeyCode::Backspace => field.backspace(),
            KeyCode::Delete => field.delete(),
            KeyCode::Left => field.left(),
            KeyCode::Right => field.right(),
            KeyCode::Home => field.start(),
            KeyCode::End => field.end(),
            _ => {}
        }
    }

    /// What to save as `id`, once every line checks; else each refused line says why. A new connection cannot take the
    /// name of one in `connections`.
    fn submit(&mut self, id: ConfigChangeId, connections: &[Connection]) -> Option<SaveConnection> {
        let name = self.checked_name(connections);
        let url = Target::parse(self.url.text()).map_err(|err| (Slot::Url, format!("{err:#}")));
        let (tunnel, password) = (self.checked_tunnel(), self.checked_password());
        let (tls, statement_timeout) = (self.checked_tls(), self.checked_statement_timeout());
        let refused = [name.as_ref().err(), url.as_ref().err(), tunnel.as_ref().err(), password.as_ref().err()];
        let refused = refused.into_iter().chain([tls.as_ref().err(), statement_timeout.as_ref().err()]);
        self.errors = refused.flatten().cloned().collect();
        let (Ok(name), Ok(url), Ok(tunnel), Ok(password), Ok(tls), Ok(statement_timeout)) =
            (name, url, tunnel, password, tls, statement_timeout)
        else {
            return None;
        };
        let typed = self.take_typed_password(password.as_ref());
        let keys = Keys { url, tunnel, password, tls, statement_timeout };
        self.sent = Some(id);
        Some(SaveConnection { id, name, saving: self.saving, change: self.change(keys), password: typed })
    }

    /// The password typed, moved out of the form, for the keychain item `source` names, whether the config already
    /// names that item or not.
    fn take_typed_password(&mut self, source: Option<&SecretSource>) -> Option<KeychainPassword> {
        let has_typed = self.password.typed_count() > 0;
        match source {
            Some(SecretSource::Keychain(item)) if has_typed => {
                Some(KeychainPassword { item: item.clone(), password: std::mem::take(&mut self.password) })
            }
            _ => None,
        }
    }

    /// Every key for a new connection; for one edited, only those that differ from the config.
    fn change(&self, keys: Keys) -> ConnectionChange {
        let was = self.was.as_ref();
        ConnectionChange {
            tier: differs(self.tier, was.map(Connection::tier)),
            url: differs(keys.url, was.map(|c| c.target().clone())),
            tunnel: keys.tunnel.and_then(|tunnel| differs(tunnel, was.and_then(Connection::tunnel).cloned())),
            password: keys.password.and_then(|source| differs(source, was.and_then(Connection::password).cloned())),
            writes: differs(self.writes, was.map(Connection::writes)),
            tls: keys.tls.and_then(|tls| differs(tls, was.and_then(Connection::tls_setting))),
            statement_timeout: keys.statement_timeout.and_then(|own| differs(own, was.and_then(|c| c.statement_timeout().own()))),
        }
    }

    fn checked_name(&self, connections: &[Connection]) -> Result<ConnectionName, Refusal> {
        let name: ConnectionName = self.name.text().trim().parse().map_err(|err| (Slot::Name, format!("{err:#}")))?;
        let is_taken = self.saving == Saving::Add && connections.iter().any(|connection| *connection.name() == name);
        if is_taken {
            return Err((Slot::Name, format!("`{name}` is already a connection: `ctrl-e` in the picker edits it")));
        }
        Ok(name)
    }

    fn checked_tunnel(&self) -> Result<Option<TunnelSpec>, Refusal> {
        let was = self.was.as_ref().and_then(Connection::tunnel);
        let command = match self.tunnel.text().trim() {
            "" if was.is_some() => return Err((Slot::Tunnel, CANNOT_CLEAR.to_owned())),
            "" => return Ok(None),
            text => shell_words::split(text).context("splitting the command").map_err(|err| (Slot::Tunnel, format!("{err:#}")))?,
        };
        let before = self.checked_tunnel_before(was)?;
        TunnelSpec::new(command, before).map(Some).map_err(|err| (Slot::Tunnel, format!("{err:#}")))
    }

    fn checked_tunnel_before(&self, was: Option<&TunnelSpec>) -> Result<Option<Vec<String>>, Refusal> {
        let had = was.is_some_and(|tunnel| tunnel.before().is_some());
        match self.tunnel_before.text().trim() {
            "" if had => Err((Slot::TunnelBefore, CANNOT_CLEAR.to_owned())),
            "" => Ok(None),
            text => shell_words::split(text).map(Some).map_err(|err| (Slot::TunnelBefore, format!("splitting the command: {err}"))),
        }
    }

    fn checked_password(&self) -> Result<Option<SecretSource>, Refusal> {
        let had = self.was.as_ref().is_some_and(|c| c.password().is_some());
        let text = self.password_where.text().trim();
        let source = match self.password_from {
            PasswordFrom::Nowhere if had => Err(anyhow!(CANNOT_CLEAR)),
            PasswordFrom::Nowhere => Ok(None),
            PasswordFrom::Keychain => text.parse::<KeychainItem>().map(|item| Some(SecretSource::Keychain(item))),
            PasswordFrom::Env => Ok(Some(SecretSource::Env(text.to_owned()))),
            PasswordFrom::Command => {
                shell_words::split(text).context("splitting the command").map(|argv| Some(SecretSource::Command(argv)))
            }
        };
        let problem = source.as_ref().ok().and_then(Option::as_ref).and_then(SecretSource::problem);
        match problem {
            Some(problem) => Err((Slot::PasswordWhere, problem.to_owned())),
            None => source.map_err(|err| (Slot::PasswordWhere, format!("{err:#}"))),
        }
    }

    fn checked_tls(&self) -> Result<Option<Tls>, Refusal> {
        let had = self.was.as_ref().and_then(Connection::tls_setting).is_some();
        match self.tls {
            None if had => Err((Slot::Tls, CANNOT_CLEAR.to_owned())),
            tls => Ok(tls),
        }
    }

    fn checked_statement_timeout(&self) -> Result<Option<StatementTimeout>, Refusal> {
        let had = self.was.as_ref().and_then(|c| c.statement_timeout().own()).is_some();
        match self.statement_timeout.text().trim() {
            "" if had => Err((Slot::StatementTimeout, CANNOT_CLEAR.to_owned())),
            "" => Ok(None),
            text => text.parse().map(Some).map_err(|err| (Slot::StatementTimeout, format!("{err:#}"))),
        }
    }
}

/// The keys the form's text lines gave, once each checks.
struct Keys {
    url: Target,
    tunnel: Option<TunnelSpec>,
    password: Option<SecretSource>,
    tls: Option<Tls>,
    statement_timeout: Option<StatementTimeout>,
}

/// A character typed as it is, not a key with `ctrl` or `alt` held.
fn is_typed(key: KeyEvent) -> bool {
    key.modifiers.difference(KeyModifiers::SHIFT).is_empty()
}

/// How `key` turns a choice line, if it does.
fn turn_of(key: KeyEvent) -> Option<Turn> {
    match key.code {
        KeyCode::Left => Some(Turn::Back),
        KeyCode::Right | KeyCode::Char(' ') => Some(Turn::Forward),
        _ => None,
    }
}

/// The value after `now` among `choices`, or before it, going round.
fn turned<T: Copy + PartialEq>(choices: &[T], now: T, turn: Turn) -> T {
    let at = choices.iter().position(|&choice| choice == now).unwrap_or_default();
    let next = match turn {
        Turn::Forward => at + 1,
        Turn::Back => at + choices.len() - 1,
    };
    choices[next % choices.len()]
}

/// `now`, unless the config already says it.
fn differs<T: PartialEq>(now: T, was: Option<T>) -> Option<T> {
    match was {
        Some(was) if was == now => None,
        _ => Some(now),
    }
}

/// The password's source and where it is, as the form's lines show them.
fn password_lines(source: Option<&SecretSource>) -> (PasswordFrom, String) {
    match source {
        None => (PasswordFrom::Nowhere, String::new()),
        Some(SecretSource::Keychain(item)) => (PasswordFrom::Keychain, item.to_string()),
        Some(SecretSource::Env(variable)) => (PasswordFrom::Env, variable.clone()),
        Some(SecretSource::Command(argv)) => (PasswordFrom::Command, shell_words::join(argv)),
    }
}

impl App {
    /// `key` in the form: its own keys move, save and close; the rest goes to the line with the cursor.
    pub(super) fn form_key(&mut self, mut form: Form, key: KeyEvent) -> Vec<Action> {
        let mut actions = vec![];
        match act::act_for(key, Pane::Form) {
            Some(Act::Down) => form.step(1),
            Some(Act::Up) => form.step(-1),
            Some(Act::Close) => return vec![],
            Some(Act::Quit) => return self.quit(),
            Some(Act::Save) if form.is_saving() => {}
            Some(Act::Save) => {
                let id = self.new_config_change();
                actions.extend(form.submit(id, &self.connections).map(Action::SaveConnection));
            }
            _ => form.type_key(key),
        }
        self.overlay = Some(Overlay::Form(Box::new(form)));
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::super::picker::tests::{ctrl, on_local, press};
    use super::super::{Answer, ConfigAnswer, Run};
    use super::*;
    use crate::fixtures::named;

    fn form(app: &App) -> &Form {
        let Some(Overlay::Form(form)) = &app.overlay else { panic!("no form") };
        form
    }

    fn key(app: &mut App, code: KeyCode) -> Vec<Action> {
        app.handle_key(KeyEvent::from(code))
    }

    fn adding(app: &mut App) {
        ctrl(app, 'o');
        ctrl(app, 'a');
    }

    fn saved(actions: Vec<Action>) -> SaveConnection {
        let [Action::SaveConnection(save)] = <[Action; 1]>::try_from(actions).unwrap() else { panic!("not a save") };
        save
    }

    #[test]
    fn a_new_connection_sets_every_key_it_has() {
        let mut app = on_local();
        adding(&mut app);
        press(&mut app, "replica");
        key(&mut app, KeyCode::Tab);
        key(&mut app, KeyCode::Right);
        key(&mut app, KeyCode::Tab);
        press(&mut app, "postgres://nina@db.example.com/shop?sslmode=require");
        let save = saved(key(&mut app, KeyCode::Enter));
        assert_eq!((save.name.to_string(), save.saving), ("replica".to_owned(), Saving::Add));
        assert_eq!((save.change.tier, save.change.writes), (Some(Tier::Staging), Some(WritePolicy::ReadOnly)));
        assert_eq!(save.change.url.map(|url| url.to_string()).as_deref(), Some("postgres://nina@db.example.com/shop?sslmode=require"));
        assert!(form(&app).is_saving());
    }

    #[test]
    fn each_refused_line_says_why_and_nothing_is_saved() {
        let mut app = on_local();
        adding(&mut app);
        press(&mut app, "my shop");
        assert_eq!(key(&mut app, KeyCode::Enter), []);
        let refused: Vec<Slot> = form(&app).errors.iter().map(|(slot, _)| *slot).collect();
        assert_eq!(refused, [Slot::Name, Slot::Url]);
    }

    #[test]
    fn an_edit_writes_only_the_keys_that_changed() {
        let mut app = on_local();
        ctrl(&mut app, 'o');
        ctrl(&mut app, 'e');
        for _ in 0..4 {
            key(&mut app, KeyCode::Tab);
        }
        key(&mut app, KeyCode::Right);
        let save = saved(key(&mut app, KeyCode::Enter));
        let expected = ConnectionChange { writes: Some(WritePolicy::Allowed), ..ConnectionChange::default() };
        assert_eq!((save.name.to_string(), save.saving, save.change), ("local".to_owned(), Saving::Edit, expected));
    }

    #[test]
    fn a_keychain_password_is_typed_hidden_and_sent_with_the_save() {
        let mut app = on_local();
        ctrl(&mut app, 'o');
        ctrl(&mut app, 'e');
        key(&mut app, KeyCode::Tab);
        key(&mut app, KeyCode::Tab);
        key(&mut app, KeyCode::Right);
        key(&mut app, KeyCode::Tab);
        press(&mut app, "pgbg/local");
        key(&mut app, KeyCode::Tab);
        press(&mut app, "hunter2");
        assert!(!format!("{:?}", app.overlay).contains("hunter2"));
        let save = saved(key(&mut app, KeyCode::Enter));
        assert_eq!(save.password.map(|typed| typed.password.expose().to_owned()).as_deref(), Some("hunter2"));
        assert_eq!(save.change.password, Some(SecretSource::Keychain("pgbg/local".parse().unwrap())));
    }

    #[test]
    fn a_tunnel_without_its_port_is_refused_beside_its_line() {
        let mut app = on_local();
        ctrl(&mut app, 'o');
        ctrl(&mut app, 'e');
        for _ in 0..3 {
            key(&mut app, KeyCode::Tab);
        }
        press(&mut app, "ssh -N bastion");
        key(&mut app, KeyCode::Enter);
        assert!(form(&app).error(Slot::Tunnel).is_some_and(|why| why.contains("{port}")));
    }

    fn failed(why: &str, password: Option<Secret>) -> ConfigAnswer {
        ConfigAnswer::Failed { why: why.into(), password }
    }

    fn keychain_password_typed(app: &mut App) -> SaveConnection {
        ctrl(app, 'o');
        ctrl(app, 'e');
        to(app, Slot::PasswordFrom);
        key(app, KeyCode::Right);
        to(app, Slot::PasswordWhere);
        press(app, "pgbg/local");
        to(app, Slot::Password);
        press(app, "hunter2");
        saved(key(app, KeyCode::Enter))
    }

    #[test]
    fn the_password_typed_moves_into_the_save() {
        let mut app = on_local();
        keychain_password_typed(&mut app);
        assert_eq!(form(&app).password.typed_count(), 0);
    }

    #[test]
    fn a_failed_save_gives_the_password_typed_back_to_the_form() {
        let mut app = on_local();
        let save = keychain_password_typed(&mut app);
        app.apply(Answer::Config { id: save.id, answer: failed("permission denied", save.password.map(|typed| typed.password)) });
        let again = saved(key(&mut app, KeyCode::Enter));
        assert_eq!(again.password.map(|typed| typed.password.expose().to_owned()).as_deref(), Some("hunter2"));
    }

    #[test]
    fn a_failed_save_keeps_a_password_typed_while_it_was_saving() {
        let mut app = on_local();
        let save = keychain_password_typed(&mut app);
        press(&mut app, "swordfish");
        app.apply(Answer::Config { id: save.id, answer: failed("permission denied", save.password.map(|typed| typed.password)) });
        assert_eq!(form(&app).password.expose(), "swordfish");
    }

    fn local() -> Connection {
        named("local", "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\n")
    }

    fn adding_replica(app: &mut App) -> SaveConnection {
        adding(app);
        press(app, "replica");
        key(app, KeyCode::Tab);
        key(app, KeyCode::Tab);
        press(app, "postgres://nina@localhost/shop");
        saved(key(app, KeyCode::Enter))
    }

    #[test]
    fn a_failed_save_says_why_and_a_saved_one_closes_the_form() {
        let mut app = on_local();
        let save = adding_replica(&mut app);
        app.apply(Answer::Config { id: save.id, answer: failed("permission denied", None) });
        assert_eq!(form(&app).failure.as_deref(), Some("permission denied"));
        let again = saved(key(&mut app, KeyCode::Enter));
        app.apply(Answer::Config { id: again.id, answer: ConfigAnswer::Changed { connections: vec![local()], warning: None } });
        assert_eq!(app.overlay, None);
    }

    #[test]
    fn a_second_enter_while_saving_sends_nothing() {
        let mut app = on_local();
        adding_replica(&mut app);
        assert_eq!(key(&mut app, KeyCode::Enter), []);
    }

    #[test]
    fn an_answer_to_another_change_leaves_the_form_waiting() {
        let mut app = on_local();
        let save = adding_replica(&mut app);
        let other = save.id.next();
        app.apply(Answer::Config { id: other, answer: failed("permission denied", None) });
        app.apply(Answer::Config { id: other, answer: ConfigAnswer::Changed { connections: vec![local()], warning: None } });
        assert_eq!((form(&app).is_saving(), form(&app).failure.as_ref()), (true, None));
    }

    #[test]
    fn a_failed_save_never_touches_the_result() {
        let mut app = on_local();
        let save = adding_replica(&mut app);
        app.apply(Answer::Config { id: save.id, answer: failed("permission denied", None) });
        assert_eq!(app.sheet().run, Run::Idle);
    }

    #[test]
    fn a_name_already_taken_is_refused_beside_its_line() {
        let mut app = on_local();
        adding(&mut app);
        press(&mut app, "prod");
        key(&mut app, KeyCode::Tab);
        key(&mut app, KeyCode::Tab);
        press(&mut app, "postgres://nina@localhost/shop");
        assert_eq!(key(&mut app, KeyCode::Enter), []);
        assert!(form(&app).error(Slot::Name).is_some_and(|why| why.contains("already")), "{:?}", form(&app).errors);
    }

    fn to(app: &mut App, slot: Slot) {
        for _ in 0..form(app).slots().len() {
            if form(app).at == slot {
                return;
            }
            key(app, KeyCode::Tab);
        }
        panic!("no line {slot:?} in {:?}", form(app).slots());
    }

    fn adding_tunnelled(app: &mut App) {
        adding(app);
        press(app, "bastioned");
        to(app, Slot::Url);
        press(app, "postgres://nina@localhost/shop");
        to(app, Slot::Tunnel);
        press(app, "ssh -N -L 127.0.0.1:{port}:localhost:5432 bastion");
    }

    #[test]
    fn a_paste_lands_on_the_line_with_the_cursor_without_its_control_characters() {
        let mut app = on_local();
        adding(&mut app);
        press(&mut app, "rep");
        app.paste("li\u{1b}ca\u{202e}\n");
        assert_eq!(form(&app).name.text(), "replica");
    }

    #[test]
    fn a_paste_on_the_password_line_goes_into_the_password() {
        let mut app = on_local();
        ctrl(&mut app, 'o');
        ctrl(&mut app, 'e');
        to(&mut app, Slot::PasswordFrom);
        key(&mut app, KeyCode::Right);
        to(&mut app, Slot::PasswordWhere);
        press(&mut app, "pgbg/local");
        to(&mut app, Slot::Password);
        app.paste("hunter2\r\n");
        let save = saved(key(&mut app, KeyCode::Enter));
        assert_eq!(save.password.map(|typed| typed.password.expose().to_owned()).as_deref(), Some("hunter2"));
    }

    #[test]
    fn a_pasted_password_keeps_its_tabs_and_control_characters_but_its_line_end() {
        let mut app = on_local();
        ctrl(&mut app, 'o');
        ctrl(&mut app, 'e');
        to(&mut app, Slot::PasswordFrom);
        key(&mut app, KeyCode::Right);
        to(&mut app, Slot::PasswordWhere);
        press(&mut app, "pgbg/local");
        to(&mut app, Slot::Password);
        app.paste("hun\tter\u{1b}2\r\n");
        let save = saved(key(&mut app, KeyCode::Enter));
        assert_eq!(save.password.map(|typed| typed.password.expose().to_owned()).as_deref(), Some("hun\tter\u{1b}2"));
    }

    #[test]
    fn a_new_connection_through_a_tunnel_may_talk_in_clear_on_purpose() {
        let mut app = on_local();
        adding_tunnelled(&mut app);
        to(&mut app, Slot::Tls);
        key(&mut app, KeyCode::Left);
        let save = saved(key(&mut app, KeyCode::Enter));
        assert_eq!(save.change.tls, Some(Tls::Off));
        assert!(save.change.tunnel.is_some());
    }

    #[test]
    fn a_tunnel_saves_the_command_it_runs_before() {
        let mut app = on_local();
        adding_tunnelled(&mut app);
        to(&mut app, Slot::TunnelBefore);
        press(&mut app, "wake bastion");
        let save = saved(key(&mut app, KeyCode::Enter));
        let before = save.change.tunnel.and_then(|tunnel| tunnel.before().map(<[String]>::to_vec));
        assert_eq!(before, Some(vec!["wake".into(), "bastion".into()]));
    }

    #[test]
    fn a_statement_timeout_typed_is_saved() {
        let mut app = on_local();
        ctrl(&mut app, 'o');
        ctrl(&mut app, 'e');
        to(&mut app, Slot::StatementTimeout);
        press(&mut app, "2min");
        let save = saved(key(&mut app, KeyCode::Enter));
        assert_eq!(save.change, ConnectionChange { statement_timeout: Some("2min".parse().unwrap()), ..ConnectionChange::default() });
    }

    #[test]
    fn a_misspelt_statement_timeout_is_refused_beside_its_line() {
        let mut app = on_local();
        ctrl(&mut app, 'o');
        ctrl(&mut app, 'e');
        to(&mut app, Slot::StatementTimeout);
        press(&mut app, "soon");
        assert_eq!(key(&mut app, KeyCode::Enter), []);
        assert!(form(&app).error(Slot::StatementTimeout).is_some());
    }

    #[test]
    fn an_edit_cannot_take_out_a_statement_timeout_the_config_has() {
        let timed = named("local", "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\nstatement_timeout = \"5s\"\n");
        let mut app = App::new(vec![timed.clone()], &timed, vtui::theme::Theme::default());
        ctrl(&mut app, 'o');
        ctrl(&mut app, 'e');
        to(&mut app, Slot::StatementTimeout);
        assert_eq!(form(&app).statement_timeout.text(), "5s");
        key(&mut app, KeyCode::Backspace);
        key(&mut app, KeyCode::Backspace);
        assert_eq!(key(&mut app, KeyCode::Enter), []);
        assert!(form(&app).error(Slot::StatementTimeout).is_some_and(|why| why.contains("config.toml")));
    }

    #[test]
    fn an_alt_letter_types_nothing() {
        let mut app = on_local();
        adding(&mut app);
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT));
        assert_eq!(form(&app).name.text(), "");
    }

    #[test]
    fn a_new_password_for_the_keychain_item_the_config_names_is_sent_for_that_item() {
        let keyed =
            named("local", "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\npassword = { keychain = \"pgbg/local\" }\n");
        let mut app = App::new(vec![keyed.clone()], &keyed, vtui::theme::Theme::default());
        ctrl(&mut app, 'o');
        ctrl(&mut app, 'e');
        for _ in 0..4 {
            key(&mut app, KeyCode::Tab);
        }
        press(&mut app, "hunter3");
        let save = saved(key(&mut app, KeyCode::Enter));
        assert_eq!(save.change.password, None);
        let typed = save.password.map(|typed| (typed.item.to_string(), typed.password.expose().to_owned()));
        assert_eq!(typed, Some(("pgbg/local".to_owned(), "hunter3".to_owned())));
    }
}
