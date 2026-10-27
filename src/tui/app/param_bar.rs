//! The parameter bar above the editor: one field per `{{name}}` of the statement under the cursor.
use super::editor;
use crate::db::model::ParamValue;
use crate::query::params::{Given, ParamName};
use crossterm::event::{KeyCode, KeyEvent};
use std::collections::BTreeMap;
use vtui::field::Field;

/// The values typed in the bar, by name: they stay on the sheet until pgbg quits, and are never written anywhere.
#[derive(Debug, Default, PartialEq)]
pub(in crate::tui) struct ParamBar {
    fields: BTreeMap<ParamName, Field>,
    /// The field with the keys, by its place among the statement's names.
    pub(in crate::tui) at: usize,
}

impl ParamBar {
    /// The field of `name`, empty until something is typed in it.
    pub(in crate::tui) fn field(&self, name: &ParamName) -> Option<&Field> {
        self.fields.get(name)
    }

    /// Moves to the next field among `count`, or to the one before; false past either end, where the keys leave the bar.
    pub(super) fn step(&mut self, count: usize, direction: Direction) -> bool {
        let next = match direction {
            Direction::Forward => self.at.checked_add(1).filter(|&next| next < count),
            Direction::Back => self.at.checked_sub(1),
        };
        let Some(next) = next else { return false };
        self.at = next;
        true
    }

    /// Applies `key` to the field of `name`; a value holds one line, so `⏎` with any modifier types nothing.
    pub(super) fn edit(&mut self, name: &ParamName, key: KeyEvent) {
        if key.code == KeyCode::Enter {
            return;
        }
        editor::apply(self.field_mut(name), key);
    }

    /// Pastes `text` in the field of `name`, its line ends as spaces.
    pub(super) fn paste(&mut self, name: &ParamName, text: &str) {
        self.field_mut(name).paste(&text.replace(['\r', '\n'], " "));
    }

    /// The field of `name`, made on first use as a secret one, so a value never shows in `Debug` or a log.
    fn field_mut(&mut self, name: &ParamName) -> &mut Field {
        self.fields.entry(name.clone()).or_insert_with(|| Field::secret(""))
    }

    /// The values typed for `names`; a name left empty falls back to its header's default.
    pub(super) fn given(&self, names: &[ParamName]) -> Given {
        let typed = |name: &ParamName| self.fields.get(name).map(|field| (name.clone(), ParamValue::new(field.text())));
        Given::typed(names.iter().filter_map(typed).collect())
    }
}

/// Which way `tab` and `shift-tab` go through the fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Direction {
    Forward,
    Back,
}

#[cfg(test)]
mod tests {
    use super::super::tests::typed;
    use super::super::{Action, App, Focus, RunId};
    use super::*;
    use crate::query::params::ParamInput;
    use crossterm::event::KeyModifiers;

    fn press(app: &mut App, code: KeyCode) -> Vec<Action> {
        app.handle_key(KeyEvent::from(code))
    }

    fn type_in(app: &mut App, text: &str) {
        text.chars().for_each(|c| drop(press(app, KeyCode::Char(c))));
    }

    fn input_of(actions: &[Action]) -> Option<&ParamInput> {
        match actions {
            [Action::Run { input, .. }] => Some(input),
            _ => None,
        }
    }

    const SINCE_AND_OWNER: &str = "-- param: since date = 2024-01-01\nSELECT * FROM orders WHERE day > {{since}} AND owner = {{owner}}";

    #[test]
    fn tab_from_the_editor_gives_the_keys_to_the_first_parameter() {
        let mut app = typed(SINCE_AND_OWNER);
        press(&mut app, KeyCode::Tab);
        assert_eq!((app.focus, app.sheet().param_at()), (Focus::Params, Some(name("since"))));
    }

    #[test]
    fn tab_skips_the_bar_when_the_statement_has_no_parameter() {
        let mut app = typed("SELECT 1");
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Result);
    }

    #[test]
    fn enter_in_the_bar_runs_with_the_values_typed_and_the_header() {
        let mut app = typed(SINCE_AND_OWNER);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        type_in(&mut app, "nina");
        let actions = press(&mut app, KeyCode::Enter);
        let input = input_of(&actions).unwrap();
        assert_eq!(input.given, Given::typed([(name("owner"), ParamValue::new("nina"))].into()));
        assert_eq!(input.specs.len(), 1);
    }

    #[test]
    fn tab_past_the_last_parameter_goes_to_the_result_and_shift_tab_before_the_first_to_the_editor() {
        let mut app = typed(SINCE_AND_OWNER);
        press(&mut app, KeyCode::Tab);
        app.handle_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(app.focus, Focus::Editor);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Result);
    }

    #[test]
    fn esc_in_the_bar_goes_back_to_the_editor_and_keeps_the_values() {
        let mut app = typed(SINCE_AND_OWNER);
        press(&mut app, KeyCode::Tab);
        type_in(&mut app, "2025");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.focus, Focus::Editor);
        assert_eq!(app.sheet().bar.field(&name("since")).map(Field::text), Some("2025"));
    }

    #[test]
    fn esc_in_the_bar_while_a_statement_runs_cancels_it_and_keeps_the_keys() {
        let mut app = typed(SINCE_AND_OWNER);
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Enter);
        let connection = app.sheet().name().clone();
        assert_eq!(press(&mut app, KeyCode::Esc), [Action::Cancel { connection, run: RunId::FIRST }]);
        assert_eq!(app.focus, Focus::Params);
    }

    #[test]
    fn a_modified_enter_types_no_line_end_in_a_value() {
        let mut app = typed(SINCE_AND_OWNER);
        press(&mut app, KeyCode::Tab);
        type_in(&mut app, "2025");
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        assert_eq!(app.sheet().bar.field(&name("since")).map(Field::text), Some("2025"));
    }

    fn name(text: &str) -> ParamName {
        text.parse().unwrap()
    }

    #[test]
    fn the_debug_of_the_app_never_shows_a_value_typed_or_pasted() {
        let mut app = typed(SINCE_AND_OWNER);
        press(&mut app, KeyCode::Tab);
        type_in(&mut app, "2025-03-01");
        press(&mut app, KeyCode::Tab);
        app.paste("maria");
        let printed = format!("{app:?}");
        assert!(!printed.contains("2025-03-01") && !printed.contains("maria"), "{printed}");
    }

    #[test]
    fn a_paste_keeps_the_value_on_one_line() {
        let mut bar = ParamBar::default();
        bar.paste(&name("owner"), "nina\nmaria");
        assert_eq!(bar.field(&name("owner")).map(Field::text), Some("nina maria"));
    }
}
