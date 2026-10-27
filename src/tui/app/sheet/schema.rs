//! The sheet's schema: read the first time something needs it, again on demand, and the completion index built on it.
use super::super::Action;
use super::{SessionState, Sheet};
use crate::complete::Index;
use crate::connection::SchemaReading;
use crate::db::model::Catalog;
use crate::failure::Failure;
use std::sync::Arc;

/// The state of the sheet's schema: read the first time it is needed, and again on demand.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(in crate::tui) enum SchemaState {
    #[default]
    NotRead,
    Reading,
    /// Shared with whatever builds on it, and never changed: a new read replaces it whole.
    Read(Arc<Catalog>),
    /// The read failed, with its message; the session stays as it was.
    Failed(String),
}

impl Sheet {
    /// The schema last read, if any.
    pub(in crate::tui) fn catalog(&self) -> Option<&Catalog> {
        match &self.schema {
            SchemaState::Read(catalog) => Some(catalog),
            _ => None,
        }
    }

    /// Asks for the schema the first time something needs it; does nothing once asked.
    pub(in crate::tui::app) fn want_schema(&mut self) -> Vec<Action> {
        if self.schema != SchemaState::NotRead {
            return vec![];
        }
        self.read_schema_again()
    }

    /// Asks for the schema again, unless a read is on its way, the session is closed, or the connection says
    /// `schema = "off"`; a failed session opens again to read it.
    pub(in crate::tui::app) fn read_schema_again(&mut self) -> Vec<Action> {
        let is_off = self.connection.schema() == SchemaReading::Off;
        let is_closed = self.session == SessionState::Closed;
        if is_off || is_closed || self.schema == SchemaState::Reading {
            return vec![];
        }
        self.schema = SchemaState::Reading;
        if matches!(self.session, SessionState::Failed(_)) {
            self.session = SessionState::Opening;
        }
        vec![Action::ReadSchema { connection: self.name().clone(), session: self.session_id }]
    }

    /// Keeps `index` when the catalog it was built from is still the latest one read.
    pub(super) fn take_index(&mut self, catalog: &Arc<Catalog>, index: Arc<Index>) {
        let is_current = matches!(&self.schema, SchemaState::Read(read) if Arc::ptr_eq(read, catalog));
        if is_current {
            self.index = Some(index);
        }
    }

    /// Fails a schema read that waits on a session that did not open or was lost, so it can be asked for again.
    pub(super) fn fail_schema_read(&mut self, why: &str) {
        if self.schema == SchemaState::Reading {
            self.schema = SchemaState::Failed(why.to_owned());
        }
    }

    /// Keeps the schema read, or why it failed; the index of the last one is dropped.
    pub(super) fn take_schema(&mut self, read: Result<Arc<Catalog>, Failure>) {
        self.schema = read.map_or_else(|failure| SchemaState::Failed(failure.to_string()), SchemaState::Read);
        self.index = None;
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::fixtures::{app, app_on, shop, to_shop};
    use super::super::super::{Incoming, SessionId};
    use super::*;
    use crate::failure::Lost;
    use crate::fixtures::{self, shop_catalog};

    fn read_schema() -> Vec<Action> {
        vec![Action::ReadSchema { connection: shop(), session: SessionId::FIRST }]
    }

    fn catalog() -> Arc<Catalog> {
        Arc::new(shop_catalog())
    }

    #[test]
    fn the_schema_is_asked_for_the_first_time_it_is_needed() {
        assert_eq!(app().sheet_mut().want_schema(), read_schema());
    }

    #[test]
    fn the_schema_is_asked_for_once() {
        let mut app = app();
        app.sheet_mut().want_schema();
        assert_eq!(app.sheet_mut().want_schema(), []);
    }

    #[test]
    fn a_connection_whose_schema_is_off_never_asks_for_it() {
        let off = fixtures::connection("tier = \"local\"\nurl = \"postgres://nina@localhost/shop\"\nschema = \"off\"\n");
        assert_eq!(app_on(&off).sheet_mut().want_schema(), []);
    }

    #[test]
    fn the_schema_read_is_kept_on_the_sheet() {
        let mut app = app();
        app.sheet_mut().want_schema();
        app.apply(to_shop(Incoming::Schema(Ok(catalog()))));
        assert_eq!(app.sheet().schema, SchemaState::Read(catalog()));
    }

    #[test]
    fn a_schema_that_could_not_be_read_says_why() {
        let mut app = app();
        app.sheet_mut().want_schema();
        app.apply(to_shop(Incoming::Schema(Err(Failure::Server {
            message: "permission denied for table pg_class".into(),
            position: None,
        }))));
        assert_eq!(app.sheet().schema, SchemaState::Failed("permission denied for table pg_class".to_owned()));
    }

    #[test]
    fn a_schema_read_can_be_read_again() {
        let mut app = app();
        app.sheet_mut().want_schema();
        app.apply(to_shop(Incoming::Schema(Ok(catalog()))));
        assert_eq!(app.sheet_mut().read_schema_again(), read_schema());
    }

    #[test]
    fn a_schema_being_read_is_not_asked_for_again() {
        let mut app = app();
        app.sheet_mut().want_schema();
        assert_eq!(app.sheet_mut().read_schema_again(), []);
    }

    #[test]
    fn a_sheet_opened_again_asks_its_new_session_for_the_schema() {
        let mut app = app();
        app.sheet_mut().want_schema();
        app.apply(to_shop(Incoming::Schema(Ok(catalog()))));
        let connection = app.sheet().connection.clone();
        app.sheet_mut().reopen(connection, SessionId::FIRST.next());
        assert_eq!(app.sheet_mut().want_schema(), [Action::ReadSchema { connection: shop(), session: SessionId::FIRST.next() }]);
    }

    #[test]
    fn a_sheet_closed_while_its_schema_is_read_asks_again_once_it_opens() {
        let mut app = app();
        app.sheet_mut().want_schema();
        app.sheet_mut().close();
        let connection = app.sheet().connection.clone();
        app.sheet_mut().reopen(connection, SessionId::FIRST.next());
        assert_eq!(app.sheet_mut().want_schema(), [Action::ReadSchema { connection: shop(), session: SessionId::FIRST.next() }]);
    }

    #[test]
    fn a_session_that_does_not_open_fails_the_schema_waiting_on_it() {
        let mut app = app();
        app.sheet_mut().want_schema();
        app.apply(to_shop(Incoming::NotOpened(Failure::Login("password authentication failed for user \"nina\"".into()))));
        assert_eq!(app.sheet().schema, SchemaState::Failed("password authentication failed for user \"nina\"".to_owned()));
    }

    #[test]
    fn a_session_lost_while_its_schema_is_read_fails_the_read_so_it_can_be_asked_again() {
        let mut app = app();
        app.sheet_mut().want_schema();
        app.apply(to_shop(Incoming::Closed(Lost::LinkFailed("terminating connection".into()))));
        assert_eq!(app.sheet_mut().read_schema_again(), read_schema());
    }

    #[test]
    fn asking_for_the_schema_on_a_failed_session_opens_it_again() {
        let mut app = app();
        app.apply(to_shop(Incoming::NotOpened(Failure::Login("connection refused".into()))));
        app.sheet_mut().want_schema();
        assert_eq!(app.sheet().session, SessionState::Opening);
    }

    #[test]
    fn a_closed_sheet_asks_for_no_schema() {
        let mut app = app();
        app.sheet_mut().close();
        assert_eq!(app.sheet_mut().want_schema(), []);
    }
}
