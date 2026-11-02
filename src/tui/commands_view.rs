//! The `ctrl-l` command palette: a vtui palette over every act and theme, what runs from the pane on top.
use super::app::{App, Command, Overlay};
use super::palette_footer;
use super::theme::Theme;
use crate::act::Pane;
use ratatui::Frame;
use vtui::palette::MAX_SHOWN;
use vtui::palette::view::{self, Pick, Popup};

/// A row's label and its faded detail: the keys, then the pane they work in when not this one.
fn written(command: &Command) -> (String, String) {
    match command {
        Command::Act(listing) if listing.runs => (listing.does.clone(), listing.keys.clone()),
        Command::Act(listing) => (listing.does.clone(), format!("{} · {}", listing.keys, listing.pane.title())),
        Command::Theme(palette) => (format!("theme {}", palette.name), String::new()),
    }
}

fn pick<'a>(theme: &Theme, command: &Command, (label, detail): &'a (String, String)) -> Pick<'a> {
    let (icon, colour) = match command {
        Command::Act(listing) if listing.runs => ("›", theme.accent),
        Command::Act(_) => ("·", theme.faded),
        Command::Theme(_) => ("◐", theme.accent),
    };
    Pick { icon, colour, label, detail: view::faded(theme, detail) }
}

/// The palette over the whole screen when it is open.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) {
    let Some(Overlay::Commands(commands)) = app.overlay() else { return };
    let theme = app.theme();
    let search = &commands.search;
    let in_view = search.in_view(MAX_SHOWN);
    let written: Vec<(String, String)> = in_view.iter().map(written).collect();
    let picks: Vec<Pick<'_>> = in_view.iter().zip(&written).map(|(command, text)| pick(theme, command, text)).collect();
    let selected = search.palette.selected - search.first_in_view(MAX_SHOWN);
    let footer = palette_footer::text(Pane::Commands, f.area());
    let popup = Popup { title: "Commands", prompt: view::prompt(theme, &search.palette, "", |_| vec![]), footer: &footer };
    view::draw(f, theme, f.area(), popup, |room| view::pick_lines(theme, &picks, selected, room));
}

#[cfg(test)]
mod tests {
    use super::super::app::fixtures::{app, app_on, connection, on_the_result, opened_on};
    use super::super::ui;
    use super::App;
    use crate::connection::Tier;
    use crate::fixtures::orders;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(app: &App) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, app)).unwrap();
        terminal
    }

    #[test]
    fn the_palette_lists_what_runs_from_the_editor_with_its_keys() {
        let app = opened_on(app(), "");
        insta::assert_snapshot!("commands", render(&app).backend());
    }

    #[test]
    fn typing_narrows_the_list_and_what_runs_elsewhere_names_its_pane() {
        let app = opened_on(on_the_result(orders()), "copy");
        insta::assert_snapshot!("commands_filtered", render(&app).backend());
    }

    #[test]
    fn a_list_with_no_match_says_so() {
        let app = opened_on(app(), "zzzz");
        insta::assert_snapshot!("commands_no_match", render(&app).backend());
    }

    #[test]
    fn a_previewed_theme_paints_the_tui_and_the_frame_keeps_the_tier_colour_of_that_theme() {
        let app = opened_on(app_on(&connection("production", "read-only")), "theme nord");
        let terminal = render(&app);
        assert_eq!(app.theme().name, "nord");
        assert_eq!(terminal.backend().buffer()[(0, 0)].fg, app.theme().tier(Tier::Production));
        insta::assert_snapshot!("commands_theme", terminal.backend());
    }
}
