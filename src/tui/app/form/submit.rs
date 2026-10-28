//! What the form sends once every line checks: the keys that changed, and the password typed for the keychain.
use super::{Form, PasswordFrom, RefusedLine, Slot};
use crate::config::ConnectionChange;
use crate::connection::{Connection, ConnectionName, StatementTimeout, Target, Tls, TunnelSpec};
use crate::secret::{KeychainItem, SecretSource};
use crate::tui::app::{ConfigChangeId, KeychainPassword, SaveConnection, Saving};
use anyhow::{Context, anyhow};

const CANNOT_CLEAR: &str = "pgbg cannot take a key out of the config yet: edit config.toml for that";

/// The keys the form's text lines gave, once each checks.
struct Keys {
    url: Target,
    tunnel: Option<TunnelSpec>,
    password: Option<SecretSource>,
    tls: Option<Tls>,
    statement_timeout: Option<StatementTimeout>,
}

/// `now` when it differs from `was`, the value the config already holds.
fn differs<T: PartialEq>(now: T, was: Option<T>) -> Option<T> {
    match was {
        Some(was) if was == now => None,
        _ => Some(now),
    }
}

impl Form {
    /// What to save as `id` once every line checks; otherwise each refused line says why.
    /// A new connection cannot reuse the name of one in `connections`.
    pub(super) fn submit(&mut self, id: ConfigChangeId, connections: &[Connection]) -> Option<SaveConnection> {
        let name = self.checked_name(connections);
        let url = Target::parse(self.url.text()).map_err(|err| RefusedLine::new(Slot::Url, format!("{err:#}")));
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

    /// Moves the typed password out of the form, for the keychain item `source` names, whether or not the config
    /// already names that item.
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
            queries: None,
        }
    }

    fn checked_name(&self, connections: &[Connection]) -> Result<ConnectionName, RefusedLine> {
        let name: ConnectionName = self.name.text().trim().parse().map_err(|err| RefusedLine::new(Slot::Name, format!("{err:#}")))?;
        let is_taken = self.saving == Saving::Add && connections.iter().any(|connection| *connection.name() == name);
        if is_taken {
            return Err(RefusedLine::new(Slot::Name, format!("`{name}` is already a connection: `ctrl-e` in the picker edits it")));
        }
        Ok(name)
    }

    fn checked_tunnel(&self) -> Result<Option<TunnelSpec>, RefusedLine> {
        let was = self.was.as_ref().and_then(Connection::tunnel);
        let command = match self.tunnel.text().trim() {
            "" if was.is_some() => return Err(RefusedLine::new(Slot::Tunnel, CANNOT_CLEAR)),
            "" => return Ok(None),
            text => shell_words::split(text)
                .context("splitting the command")
                .map_err(|err| RefusedLine::new(Slot::Tunnel, format!("{err:#}")))?,
        };
        let before = self.checked_tunnel_before(was)?;
        TunnelSpec::new(command, before).map(Some).map_err(|err| RefusedLine::new(Slot::Tunnel, format!("{err:#}")))
    }

    fn checked_tunnel_before(&self, was: Option<&TunnelSpec>) -> Result<Option<Vec<String>>, RefusedLine> {
        let had = was.is_some_and(|tunnel| tunnel.before().is_some());
        match self.tunnel_before.text().trim() {
            "" if had => Err(RefusedLine::new(Slot::TunnelBefore, CANNOT_CLEAR)),
            "" => Ok(None),
            text => shell_words::split(text)
                .map(Some)
                .map_err(|err| RefusedLine::new(Slot::TunnelBefore, format!("splitting the command: {err}"))),
        }
    }

    fn checked_password(&self) -> Result<Option<SecretSource>, RefusedLine> {
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
            Some(problem) => Err(RefusedLine::new(Slot::PasswordWhere, problem)),
            None => source.map_err(|err| RefusedLine::new(Slot::PasswordWhere, format!("{err:#}"))),
        }
    }

    fn checked_tls(&self) -> Result<Option<Tls>, RefusedLine> {
        let had = self.was.as_ref().and_then(Connection::tls_setting).is_some();
        match self.tls {
            None if had => Err(RefusedLine::new(Slot::Tls, CANNOT_CLEAR)),
            tls => Ok(tls),
        }
    }

    fn checked_statement_timeout(&self) -> Result<Option<StatementTimeout>, RefusedLine> {
        let had = self.was.as_ref().and_then(|c| c.statement_timeout().own()).is_some();
        match self.statement_timeout.text().trim() {
            "" if had => Err(RefusedLine::new(Slot::StatementTimeout, CANNOT_CLEAR)),
            "" => Ok(None),
            text => text.parse().map(Some).map_err(|err| RefusedLine::new(Slot::StatementTimeout, format!("{err:#}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::picker::tests::{ctrl, on_local, press};
    use super::super::super::{Answer, App, ConfigAnswer, Run};
    use super::super::tests::*;
    use super::*;
    use crate::connection::{Tier, WritePolicy};
    use crate::fixtures::named;
    use crate::secret::Secret;
    use crossterm::event::KeyCode;

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
        let refused: Vec<Slot> = form(&app).errors.iter().map(|refused| refused.slot).collect();
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
        let mut app = App::new(vec![timed.clone()], &timed, crate::tui::theme::Theme::default());
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
    fn a_new_password_for_the_keychain_item_the_config_names_is_sent_for_that_item() {
        let keyed =
            named("local", "tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\npassword = { keychain = \"pgbg/local\" }\n");
        let mut app = App::new(vec![keyed.clone()], &keyed, crate::tui::theme::Theme::default());
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

    fn adding_replica(app: &mut App) -> SaveConnection {
        adding(app);
        press(app, "replica");
        key(app, KeyCode::Tab);
        key(app, KeyCode::Tab);
        press(app, "postgres://nina@localhost/shop");
        saved(key(app, KeyCode::Enter))
    }
}
