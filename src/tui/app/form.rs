//! The connection form, from the picker's `ctrl-a` and `ctrl-e`: each key of a connection on its own line, checked as
//! the command line checks its flags, then written by the backend; an edit writes only the keys that changed.
use super::{Action, App, ConfigChangeId, KeychainPassword, Overlay, SaveConnection, Saving};
use crate::act::{self, Act, Pane};
use crate::config::ConnectionChange;
use crate::connection::{Connection, ConnectionName, Target, Tier, TunnelSpec, WritePolicy};
use crate::secret::{KeychainItem, Secret, SecretSource};
use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use vtui::field::Field;

const CANNOT_CLEAR: &str = "pgbg cannot take a key out of the config yet: edit config.toml for that";

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
    Writes,
}

/// Where the password comes from, as the form offers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum PasswordFrom {
    Nowhere,
    Keychain,
    Env,
    Command,
}

/// The form while it is open: what each line holds, the line with the cursor, and what was refused.
#[derive(Clone, Debug, PartialEq)]
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
    pub(in crate::tui) writes: WritePolicy,
    /// Why a line is refused, each beside its line.
    pub(in crate::tui) errors: Vec<(Slot, String)>,
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
            writes: WritePolicy::ReadOnly,
            errors: vec![],
            sent: None,
            failure: None,
        }
    }

    /// The form on `connection` as the config has it.
    pub(in crate::tui) fn editing(connection: &Connection) -> Self {
        let (password_from, password_where) = password_lines(connection.password());
        Self {
            saving: Saving::Edit,
            was: Some(connection.clone()),
            at: Slot::Tier,
            name: Field::new(connection.name().to_string()),
            tier: connection.tier(),
            url: Field::new(connection.target().to_string()),
            password_from,
            password_where: Field::new(password_where),
            tunnel: Field::new(connection.tunnel().map(|tunnel| shell_words::join(tunnel.command())).unwrap_or_default()),
            writes: connection.writes(),
            ..Self::adding()
        }
    }

    /// The lines shown, in order: the name only for a new connection, the password's only where it has one.
    pub(in crate::tui) fn slots(&self) -> Vec<Slot> {
        let is_new = self.saving == Saving::Add;
        let has_where = self.password_from != PasswordFrom::Nowhere;
        let has_password = self.password_from == PasswordFrom::Keychain;
        let shown = [(Slot::Name, is_new), (Slot::Tier, true), (Slot::Url, true), (Slot::PasswordFrom, true)];
        let rest = [(Slot::PasswordWhere, has_where), (Slot::Password, has_password), (Slot::Tunnel, true), (Slot::Writes, true)];
        shown.into_iter().chain(rest).filter(|&(_, is_shown)| is_shown).map(|(slot, _)| slot).collect()
    }

    pub(in crate::tui) fn is_saving(&self) -> bool {
        self.sent.is_some()
    }

    /// Whether `id` is the save this form sent and waits on.
    pub(in crate::tui) fn is_waiting_on(&self, id: ConfigChangeId) -> bool {
        self.sent == Some(id)
    }

    /// Says why the save failed; the form stays as it was typed.
    pub(in crate::tui) fn failed(&mut self, why: String) {
        self.sent = None;
        self.failure = Some(why);
    }

    pub(in crate::tui) fn error(&self, slot: Slot) -> Option<&str> {
        self.errors.iter().find(|(refused, _)| *refused == slot).map(|(_, why)| why.as_str())
    }

    fn step(&mut self, by: isize) {
        let slots = self.slots();
        let at = slots.iter().position(|&slot| slot == self.at).unwrap_or_default();
        self.at = slots[at.saturating_add_signed(by).min(slots.len() - 1)];
    }

    /// `key` on the line with the cursor: a choice turns with `←` `→` or space, a text line edits.
    fn type_key(&mut self, key: KeyEvent) {
        let is_turn = matches!(key.code, KeyCode::Left | KeyCode::Right | KeyCode::Char(' '));
        let forward = key.code != KeyCode::Left;
        match self.at {
            Slot::Tier if is_turn => self.tier = turned(&[Tier::Local, Tier::Staging, Tier::Production], self.tier, forward),
            Slot::Writes if is_turn => self.writes = turned(&[WritePolicy::ReadOnly, WritePolicy::Allowed], self.writes, forward),
            Slot::PasswordFrom if is_turn => self.password_from = turned(&PASSWORD_FROM, self.password_from, forward),
            Slot::Password => typed_secret(&mut self.password, key),
            Slot::Name => typed(&mut self.name, key),
            Slot::Url => typed(&mut self.url, key),
            Slot::PasswordWhere => typed(&mut self.password_where, key),
            Slot::Tunnel => typed(&mut self.tunnel, key),
            Slot::Tier | Slot::Writes | Slot::PasswordFrom => {}
        }
    }

    /// What to save as `id`, once every line checks; else each refused line says why. A new connection cannot take the
    /// name of one in `connections`.
    fn submit(&mut self, id: ConfigChangeId, connections: &[Connection]) -> Option<SaveConnection> {
        let name = self.checked_name(connections);
        let url = Target::parse(self.url.text()).map_err(|err| format!("{err:#}"));
        let tunnel = self.checked_tunnel();
        let password = self.checked_password();
        let refused = [(Slot::Name, name.as_ref().err()), (Slot::Url, url.as_ref().err())];
        let refused = refused.into_iter().chain([(Slot::Tunnel, tunnel.as_ref().err()), (Slot::PasswordWhere, password.as_ref().err())]);
        self.errors = refused.filter_map(|(slot, why)| Some((slot, why?.clone()))).collect();
        let (Ok(name), Ok(url), Ok(tunnel), Ok(password)) = (name, url, tunnel, password) else { return None };
        let typed = self.typed_password(password.as_ref());
        self.sent = Some(id);
        Some(SaveConnection { id, name, saving: self.saving, change: self.change(url, tunnel, password), password: typed })
    }

    /// The password typed, for the keychain item `source` names, whether the config already names that item or not.
    fn typed_password(&self, source: Option<&SecretSource>) -> Option<KeychainPassword> {
        let has_typed = self.password.typed_count() > 0;
        match source {
            Some(SecretSource::Keychain(item)) if has_typed => {
                Some(KeychainPassword { item: item.clone(), password: self.password.clone() })
            }
            _ => None,
        }
    }

    /// Every key for a new connection; for one edited, only those that differ from the config.
    fn change(&self, url: Target, tunnel: Option<TunnelSpec>, password: Option<SecretSource>) -> ConnectionChange {
        let was = self.was.as_ref();
        ConnectionChange {
            tier: differs(self.tier, was.map(Connection::tier)),
            url: differs(url, was.map(|c| c.target().clone())),
            tunnel: tunnel.and_then(|tunnel| differs(tunnel, was.and_then(Connection::tunnel).cloned())),
            password: password.and_then(|source| differs(source, was.and_then(Connection::password).cloned())),
            writes: differs(self.writes, was.map(Connection::writes)),
            ..ConnectionChange::default()
        }
    }

    fn checked_name(&self, connections: &[Connection]) -> Result<ConnectionName, String> {
        let name: ConnectionName = self.name.text().trim().parse().map_err(|err| format!("{err:#}"))?;
        let is_taken = self.saving == Saving::Add && connections.iter().any(|connection| *connection.name() == name);
        if is_taken {
            return Err(format!("`{name}` is already a connection: `ctrl-e` in the picker edits it"));
        }
        Ok(name)
    }

    fn checked_tunnel(&self) -> Result<Option<TunnelSpec>, String> {
        let had = self.was.as_ref().is_some_and(|c| c.tunnel().is_some());
        let parsed = match self.tunnel.text().trim() {
            "" if had => Err(anyhow::anyhow!(CANNOT_CLEAR)),
            "" => Ok(None),
            text => shell_words::split(text).context("splitting the command").and_then(TunnelSpec::new).map(Some),
        };
        parsed.map_err(|err| format!("{err:#}"))
    }

    fn checked_password(&self) -> Result<Option<SecretSource>, String> {
        let had = self.was.as_ref().is_some_and(|c| c.password().is_some());
        let text = self.password_where.text().trim();
        let source = match self.password_from {
            PasswordFrom::Nowhere if had => Err(anyhow::anyhow!(CANNOT_CLEAR)),
            PasswordFrom::Nowhere => Ok(None),
            PasswordFrom::Keychain => text.parse::<KeychainItem>().map(|item| Some(SecretSource::Keychain(item))),
            PasswordFrom::Env => Ok(Some(SecretSource::Env(text.to_owned()))),
            PasswordFrom::Command => {
                shell_words::split(text).context("splitting the command").map(|argv| Some(SecretSource::Command(argv)))
            }
        };
        let problem = source.as_ref().ok().and_then(Option::as_ref).and_then(SecretSource::problem);
        match problem {
            Some(problem) => Err(problem.to_owned()),
            None => source.map_err(|err| format!("{err:#}")),
        }
    }
}

const PASSWORD_FROM: [PasswordFrom; 4] = [PasswordFrom::Nowhere, PasswordFrom::Keychain, PasswordFrom::Env, PasswordFrom::Command];

/// The value after `now` among `choices`, or before it, going round.
fn turned<T: Copy + PartialEq>(choices: &[T], now: T, forward: bool) -> T {
    let at = choices.iter().position(|&choice| choice == now).unwrap_or_default();
    let next = if forward { at + 1 } else { at + choices.len() - 1 };
    choices[next % choices.len()]
}

fn typed(field: &mut Field, key: KeyEvent) {
    match key.code {
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => field.insert(c),
        KeyCode::Backspace => field.backspace(),
        KeyCode::Delete => field.delete(),
        KeyCode::Left => field.left(),
        KeyCode::Right => field.right(),
        KeyCode::Home => field.start(),
        KeyCode::End => field.end(),
        _ => {}
    }
}

fn typed_secret(password: &mut Secret, key: KeyEvent) {
    match key.code {
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => password.push(c),
        KeyCode::Backspace => password.pop(),
        _ => {}
    }
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
        app.apply(Answer::Config { id: save.id, answer: ConfigAnswer::Failed("permission denied".into()) });
        assert_eq!(form(&app).failure.as_deref(), Some("permission denied"));
        let again = saved(key(&mut app, KeyCode::Enter));
        app.apply(Answer::Config { id: again.id, answer: ConfigAnswer::Changed(vec![local()]) });
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
        app.apply(Answer::Config { id: other, answer: ConfigAnswer::Failed("permission denied".into()) });
        app.apply(Answer::Config { id: other, answer: ConfigAnswer::Changed(vec![local()]) });
        assert_eq!((form(&app).is_saving(), form(&app).failure.as_ref()), (true, None));
    }

    #[test]
    fn a_failed_save_never_touches_the_result() {
        let mut app = on_local();
        let save = adding_replica(&mut app);
        app.apply(Answer::Config { id: save.id, answer: ConfigAnswer::Failed("permission denied".into()) });
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
