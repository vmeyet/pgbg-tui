//! Keys and pastes: which pane or overlay takes them, and the act each key does.
use super::grid::Move;
use super::param_bar::Direction;
use super::{Action, App, Focus, Overlay};
use crate::act::{self, Act, Pane};
use crossterm::event::{KeyCode, KeyEvent};
use vtui::help::Help;

/// The help after `key`: `?` lists every key, `j`/`k` scroll, any other key closes it.
fn help_after(help: Help, key: KeyEvent, pane: Pane) -> Option<Help> {
    let last_row = vtui::help::last_row(&act::sections(pane, help.every_key));
    match key.code {
        KeyCode::Char('?') if !help.every_key => Some(Help { scroll: 0, every_key: true }),
        KeyCode::Char('j') | KeyCode::Down => Some(Help { scroll: (help.scroll + 1).min(last_row), ..help }),
        KeyCode::Char('k') | KeyCode::Up => Some(Help { scroll: help.scroll.saturating_sub(1), ..help }),
        _ => None,
    }
}

impl App {
    /// Takes one key; returns what the runner should do about it.
    pub(in crate::tui) fn handle_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let actions = self.take_key(key);
        self.and_the_schema_shown(actions)
    }

    fn take_key(&mut self, key: KeyEvent) -> Vec<Action> {
        self.config_failure = None;
        self.note = None;
        if let Some(overlay) = self.overlay.take() {
            return self.overlay_key(overlay, key);
        }
        let ghost = self.sheet_mut().ghost.take();
        if let Some(act) = act::act_for(key, self.pane()) {
            if let (Act::TakeGhost, Some(ghost)) = (act, ghost) {
                self.sheet_mut().take_ghost(ghost);
                return vec![];
            }
            return self.act(act);
        }
        match self.focus {
            Focus::Editor => return self.sheet_mut().edit(key),
            Focus::Params => self.sheet_mut().edit_bar(key),
            Focus::Result | Focus::Queries => {}
        }
        vec![]
    }

    /// Takes text the terminal pasted: the editor, the finder or the form gets it whole, so its line ends run nothing.
    pub(in crate::tui) fn paste(&mut self, text: &str) -> Vec<Action> {
        match &mut self.overlay {
            Some(Overlay::Form(form)) => form.paste(text),
            Some(Overlay::Finder(finder)) => finder.paste(text, &self.queries.all),
            Some(Overlay::TableFinder(finder)) => finder.search.paste(text, &finder.all),
            Some(Overlay::Recall(recall)) => recall.search.paste(text, &recall.all),
            Some(Overlay::SchemaFilter) => self.schema_tab.filter.paste(text),
            Some(Overlay::Completion(_)) => self.paste_past_completions(text),
            Some(Overlay::Help(_) | Overlay::Picker(_) | Overlay::Prompt(_) | Overlay::Detail(_) | Overlay::CopyAs) => {}
            None => self.paste_in_focus(text),
        }
        vec![]
    }

    fn paste_in_focus(&mut self, text: &str) {
        match self.focus {
            Focus::Editor => self.sheet_mut().paste(text),
            Focus::Params => self.sheet_mut().paste_in_bar(text),
            Focus::Result | Focus::Queries => {}
        }
    }

    /// Hands `key` to `overlay`, which stays open unless the key closes it.
    fn overlay_key(&mut self, overlay: Overlay, key: KeyEvent) -> Vec<Action> {
        match overlay {
            Overlay::Help(help) => {
                self.overlay = help_after(help, key, self.pane()).map(Overlay::Help);
                vec![]
            }
            Overlay::Finder(finder) => {
                let (finder, actions) = self.finder_after(finder, key);
                if let Some(finder) = finder {
                    self.overlay = Some(Overlay::Finder(finder));
                }
                actions
            }
            Overlay::Picker(picker) => self.picker_key(picker, key),
            Overlay::Recall(recall) => self.recall_key(recall, key),
            Overlay::TableFinder(finder) => self.table_finder_key(*finder, key),
            Overlay::Form(form) => self.form_key(*form, key),
            Overlay::Prompt(prompt) => {
                let (prompt, actions) = self.prompt_after(prompt, key);
                self.overlay = prompt.map(Overlay::Prompt);
                actions
            }
            Overlay::Detail(detail) => self.detail_key(detail, key),
            Overlay::CopyAs => self.copy_as_key(key),
            Overlay::SchemaFilter => self.schema_filter_key(key),
            Overlay::Completion(completions) => self.completions_key(completions, key),
        }
    }

    fn act(&mut self, act: Act) -> Vec<Action> {
        if self.pane() == Pane::Schema
            && let Some(actions) = self.schema_act(act)
        {
            return actions;
        }
        match act {
            Act::Run => return self.run_editor(),
            Act::TakeGhost => return self.sheet_mut().edit(KeyEvent::from(KeyCode::Right)),
            Act::Complete => return self.open_completions(),
            Act::Cancel if self.focus == Focus::Params && !self.sheet().is_running() => self.focus = Focus::Editor,
            Act::Cancel => return self.sheet_mut().cancel(),
            Act::NewLine => return self.sheet_mut().edit(KeyEvent::from(KeyCode::Enter)),
            Act::SwitchPane => self.switch_pane(),
            Act::Help => self.overlay = Some(Overlay::Help(Help::default())),
            Act::Quit => return self.quit(),
            Act::Down if self.focus == Focus::Params => self.step_bar(Direction::Forward),
            Act::Up if self.focus == Focus::Params => self.step_bar(Direction::Back),
            Act::Down if self.focus == Focus::Queries => self.step_sidebar(1),
            Act::Up if self.focus == Focus::Queries => self.step_sidebar(-1),
            act if act.is_grid() && self.sheet().is_chart_shown() => {}
            Act::Down => return self.grid_step(Move::Down),
            Act::Up => return self.grid_step(Move::Up),
            Act::PageDown => return self.grid_step(Move::PageDown),
            Act::PageUp => return self.grid_step(Move::PageUp),
            Act::Top => return self.grid_step(Move::First),
            Act::Bottom => return self.grid_step(Move::Last),
            Act::Left => return self.grid_step(Move::Left),
            Act::Right => return self.grid_step(Move::Right),
            Act::Find => return self.find(),
            Act::Connections => self.open_picker(),
            Act::Chart => self.sheet_mut().switch_view(),
            Act::StepFormat => {
                let clock = self.reading.clock;
                self.sheet_mut().step_format(&clock);
            }
            Act::SwitchFormats => {
                let clock = self.reading.clock;
                self.sheet_mut().switch_every_format(&clock);
            }
            Act::Open => return self.open_in_sidebar(),
            Act::Save => return self.save(),
            Act::EditOutside => return self.edit_outside(),
            Act::History => return self.recall(),
            Act::FindTable => return self.find_table(),
            Act::Detail => self.open_detail(),
            Act::CopyCell => return self.copy_cell(),
            Act::CopyRow => return self.copy_row(),
            Act::CopyResult => self.ask_copy_format(),
            Act::SwitchTab => return self.switch_tab(),
            Act::Close | Act::Delete | Act::CloseSession | Act::Add | Act::Edit | Act::CopyCsv | Act::CopyJson | Act::CopyMarkdown => {}
            Act::Filter | Act::ReadAgain | Act::Preview | Act::Insert => {}
        }
        vec![]
    }

    /// Gives the keys to the next pane; the parameter bar takes them on its first field.
    fn switch_pane(&mut self) {
        self.focus = self.next_pane();
        if self.focus == Focus::Params {
            self.sheet_mut().bar.at = 0;
        }
    }

    /// Steps through the bar's fields; past its last one the keys go to the next pane, before its first to the editor.
    fn step_bar(&mut self, direction: Direction) {
        if self.sheet_mut().step_bar(direction) {
            return;
        }
        self.focus = match direction {
            Direction::Forward => self.next_pane(),
            Direction::Back => Focus::Editor,
        };
    }

    /// The pane `tab` goes to: the next in `Focus::ORDER` that the frame drew.
    fn next_pane(&self) -> Focus {
        let at = Focus::ORDER.iter().position(|&focus| focus == self.focus).unwrap_or_default();
        let after = Focus::ORDER.iter().cycle().skip(at + 1).take(Focus::ORDER.len());
        after.copied().find(|&focus| self.is_drawn(focus)).unwrap_or_default()
    }

    pub(super) fn is_drawn(&self, focus: Focus) -> bool {
        match focus {
            Focus::Queries => self.fit.has_sidebar,
            Focus::Params => !self.sheet().param_names().is_empty(),
            Focus::Editor | Focus::Result => true,
        }
    }

    fn grid_step(&mut self, step: Move) -> Vec<Action> {
        let room = self.fit.grid;
        self.sheet_mut().step_grid(step, room)
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::{app, enter, orders, press, shop, typed, view, with_result};
    use super::*;
    use crate::chart;
    use crate::tui::app::{Run, RunId, View};
    use crossterm::event::KeyModifiers;

    #[test]
    fn typing_goes_to_the_editor() {
        assert_eq!(typed("SELECT 1").sheet().editor.text(), "SELECT 1");
    }

    #[test]
    fn tab_moves_the_keys_to_the_result_and_back() {
        let mut app = with_result();
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Result);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.focus, Focus::Editor);
    }

    #[test]
    fn v_on_the_result_shows_its_chart_then_the_grid_again() {
        let mut app = with_result();
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('v'));
        assert_eq!(view(&app), &View::Chart(chart::infer_series(&orders())));
        press(&mut app, KeyCode::Char('v'));
        assert_eq!(view(&app), &View::Grid);
    }

    #[test]
    fn v_in_the_editor_types_a_v() {
        let mut app = with_result();
        press(&mut app, KeyCode::Char('v'));
        assert_eq!((view(&app), app.sheet().editor.text()), (&View::Grid, "SELECT 1v"));
    }

    #[test]
    fn j_on_the_result_moves_its_cursor_and_types_nothing() {
        let mut app = with_result();
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('j'));
        let Run::Done { grid, .. } = &app.sheet().run else { panic!("no result") };
        assert_eq!(grid.row, 1);
        assert_eq!(app.sheet().editor.text(), "SELECT 1");
    }

    fn on_the_chart() -> App {
        let mut app = with_result();
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('v'));
        app
    }

    #[test]
    fn j_on_the_chart_leaves_the_hidden_grid_alone() {
        let mut app = on_the_chart();
        press(&mut app, KeyCode::Char('j'));
        let Run::Done { grid, .. } = &app.sheet().run else { panic!("no result") };
        assert_eq!(grid.row, 0);
    }

    #[test]
    fn y_on_the_chart_copies_no_hidden_cell() {
        assert_eq!(on_the_chart().handle_key(KeyEvent::from(KeyCode::Char('y'))), []);
    }

    #[test]
    fn a_pasted_statement_with_line_ends_runs_nothing() {
        let mut app = app();
        assert_eq!(app.paste("DELETE FROM orders\nWHERE id = 7;"), []);
        assert_eq!((app.sheet().editor.text(), &app.sheet().run), ("DELETE FROM orders\nWHERE id = 7;", &Run::Idle));
    }

    #[test]
    fn a_paste_into_the_result_pane_types_nothing() {
        let mut app = with_result();
        press(&mut app, KeyCode::Tab);
        app.paste("jjj");
        assert_eq!(app.sheet().editor.text(), "SELECT 1");
    }

    #[test]
    fn alt_enter_breaks_the_line_and_runs_nothing() {
        let mut app = typed("SELECT *");
        assert_eq!(app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT)), []);
        assert_eq!(app.sheet().editor.text(), "SELECT *\n");
    }

    #[test]
    fn a_question_mark_in_the_editor_is_typed() {
        let app = typed("SELECT '?'");
        assert_eq!((app.sheet().editor.text(), &app.overlay), ("SELECT '?'", &None));
    }

    #[test]
    fn a_question_mark_on_the_result_opens_the_help_and_any_other_key_closes_it() {
        let mut app = with_result();
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('?'));
        assert_eq!(app.overlay, Some(Overlay::Help(Help::default())));
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, None);
        let Run::Done { grid, .. } = &app.sheet().run else { panic!("no result") };
        assert_eq!(grid.row, 0);
    }

    #[test]
    fn a_second_question_mark_lists_every_key() {
        let mut app = typed("SELECT 1");
        press(&mut app, KeyCode::F(1));
        press(&mut app, KeyCode::Char('?'));
        assert_eq!(app.overlay, Some(Overlay::Help(Help { scroll: 0, every_key: true })));
    }

    #[test]
    fn ctrl_c_while_a_statement_runs_cancels_it_and_quits() {
        let mut app = typed("SELECT pg_sleep(30)");
        enter(&mut app);
        assert_eq!(
            app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            [Action::Cancel { connection: shop(), run: RunId::FIRST }]
        );
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_c_quits() {
        let mut app = typed("SELECT 1");
        app.handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
        assert_eq!(app.sheet().editor.text(), "SELECT 1");
    }
}
