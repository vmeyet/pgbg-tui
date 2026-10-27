//! The connection form, opened from the picker with `ctrl-a` and `ctrl-e`.
//! Each connection key has its own line, checked as the command line checks its flags, then written by the backend.
//! An edit writes only the keys that changed.
use super::{Action, App, ConfigChangeId, Overlay, Saving};
use crate::act::{self, Act, Pane};
use crate::connection::{Connection, Tier, Tls, TunnelSpec, WritePolicy};
use crate::secret::{Secret, SecretSource};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use vtui::field::Field;

mod submit;

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
#[derive(Clone, Debug, PartialEq)]
struct RefusedLine {
    slot: Slot,
    why: String,
}

impl RefusedLine {
    fn new(slot: Slot, why: impl Into<String>) -> Self {
        Self { slot, why: why.into() }
    }
}

/// The form while it is open: what each line holds, the line with the cursor, and what was refused.
#[derive(Debug, PartialEq)]
pub(in crate::tui) struct Form {
    saving: Saving,
    /// The connection as the config had it, when the form edits one.
    was: Option<Connection>,
    at: Slot,
    name: Field,
    tier: Tier,
    url: Field,
    password_from: PasswordFrom,
    /// The keychain item, the variable or the command, as `password_from` says.
    password_where: Field,
    /// The password for the keychain item, written on save; left empty, the item does not change.
    password: Secret,
    tunnel: Field,
    /// The command the tunnel runs first, once.
    tunnel_before: Field,
    writes: WritePolicy,
    /// The `tls` key; none leaves it to the url's `sslmode`.
    tls: Option<Tls>,
    /// Empty means the tier's default.
    statement_timeout: Field,
    /// Why a line is refused, each beside its line.
    errors: Vec<RefusedLine>,
    /// The save on its way, until its answer comes.
    sent: Option<ConfigChangeId>,
    /// Why the last save failed.
    failure: Option<String>,
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
        self.field_of(self.at)
    }

    /// The text of `slot`, when it is a text line.
    pub(in crate::tui) fn field_of(&self, slot: Slot) -> Option<&Field> {
        match slot {
            Slot::Name => Some(&self.name),
            Slot::Url => Some(&self.url),
            Slot::PasswordWhere => Some(&self.password_where),
            Slot::Tunnel => Some(&self.tunnel),
            Slot::TunnelBefore => Some(&self.tunnel_before),
            Slot::StatementTimeout => Some(&self.statement_timeout),
            Slot::Tier | Slot::PasswordFrom | Slot::Password | Slot::Writes | Slot::Tls => None,
        }
    }

    pub(in crate::tui) fn saving(&self) -> Saving {
        self.saving
    }

    /// The line with the cursor.
    pub(in crate::tui) fn at(&self) -> Slot {
        self.at
    }

    pub(in crate::tui) fn name(&self) -> &str {
        self.name.text()
    }

    pub(in crate::tui) fn tier(&self) -> Tier {
        self.tier
    }

    pub(in crate::tui) fn password_from(&self) -> PasswordFrom {
        self.password_from
    }

    /// How many characters of a password were typed, never what they are.
    pub(in crate::tui) fn typed_password_count(&self) -> usize {
        self.password.typed_count()
    }

    pub(in crate::tui) fn writes(&self) -> WritePolicy {
        self.writes
    }

    pub(in crate::tui) fn tls(&self) -> Option<Tls> {
        self.tls
    }

    pub(in crate::tui) fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    pub(in crate::tui) fn is_saving(&self) -> bool {
        self.sent.is_some()
    }

    /// Whether `id` is the save this form sent and waits on.
    pub(in crate::tui) fn is_waiting_on(&self, id: ConfigChangeId) -> bool {
        self.sent == Some(id)
    }

    /// Shows why the save failed.
    /// The form keeps what was typed, and takes back the password it sent unless another one was typed during the save.
    pub(in crate::tui) fn failed(&mut self, why: String, password: Option<Secret>) {
        self.sent = None;
        self.failure = Some(why);
        let is_password_empty = self.password.typed_count() == 0;
        if let Some(password) = password.filter(|_| is_password_empty) {
            self.password = password;
        }
    }

    pub(in crate::tui) fn error(&self, slot: Slot) -> Option<&str> {
        self.errors.iter().find(|refused| refused.slot == slot).map(|refused| refused.why.as_str())
    }

    /// Types `text` on the line with the cursor, cleaned as vtui cleans a paste and kept on one line.
    /// The password line takes any character but a line end; a choice line takes nothing.
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

/// The value after `now` in `choices`, or before it, wrapping around at either end.
fn turned<T: Copy + PartialEq>(choices: &[T], now: T, turn: Turn) -> T {
    let at = choices.iter().position(|&choice| choice == now).unwrap_or_default();
    let next = match turn {
        Turn::Forward => at + 1,
        Turn::Back => at + choices.len() - 1,
    };
    choices[next % choices.len()]
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
pub(super) mod tests {
    use super::super::SaveConnection;
    use super::super::picker::tests::{ctrl, on_local, press};
    use super::*;
    use crate::fixtures::named;

    pub(super) fn form(app: &App) -> &Form {
        let Some(Overlay::Form(form)) = &app.overlay else { panic!("no form") };
        form
    }

    pub(super) fn key(app: &mut App, code: KeyCode) -> Vec<Action> {
        app.handle_key(KeyEvent::from(code))
    }

    pub(super) fn adding(app: &mut App) {
        ctrl(app, 'o');
        ctrl(app, 'a');
    }

    pub(super) fn saved(actions: Vec<Action>) -> SaveConnection {
        let [Action::SaveConnection(save)] = <[Action; 1]>::try_from(actions).unwrap() else { panic!("not a save") };
        save
    }

    pub(super) fn local() -> Connection {
        named("local", "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\n")
    }

    pub(super) fn to(app: &mut App, slot: Slot) {
        for _ in 0..form(app).slots().len() {
            if form(app).at == slot {
                return;
            }
            key(app, KeyCode::Tab);
        }
        panic!("no line {slot:?} in {:?}", form(app).slots());
    }

    pub(super) fn adding_tunnelled(app: &mut App) {
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
    fn an_alt_letter_types_nothing() {
        let mut app = on_local();
        adding(&mut app);
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT));
        assert_eq!(form(&app).name.text(), "");
    }
}
