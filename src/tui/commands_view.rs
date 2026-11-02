//! The `ctrl-l` command palette: a vtui palette over every act, what runs from the pane on top.
use super::app::{App, Overlay};
use super::palette_footer;
use super::theme::Theme;
use crate::act::{Listing, Pane};
use ratatui::Frame;
use vtui::palette::MAX_SHOWN;
use vtui::palette::view::{self, Pick, Popup};

/// A row's faded detail: the keys, then the pane they work in when not this one.
fn detail_of(listing: &Listing) -> String {
    if listing.runs { listing.keys.clone() } else { format!("{} · {}", listing.keys, listing.pane.title()) }
}

fn pick<'a>(theme: &Theme, listing: &'a Listing, detail: &'a str) -> Pick<'a> {
    let (icon, colour) = if listing.runs { ("›", theme.accent) } else { ("·", theme.faded) };
    Pick { icon, colour, label: &listing.does, detail: view::faded(theme, detail) }
}

/// The palette over the whole screen when it is open.
pub(super) fn draw(f: &mut Frame<'_>, app: &App) {
    let Some(Overlay::Commands(commands)) = app.overlay() else { return };
    let theme = app.theme();
    let search = &commands.search;
    let in_view = search.in_view(MAX_SHOWN);
    let details: Vec<String> = in_view.iter().map(detail_of).collect();
    let picks: Vec<Pick<'_>> = in_view.iter().zip(&details).map(|(listing, detail)| pick(theme, listing, detail)).collect();
    let selected = search.palette.selected - search.first_in_view(MAX_SHOWN);
    let footer = palette_footer::text(Pane::Commands, f.area());
    let popup = Popup { title: "Commands", prompt: view::prompt(theme, &search.palette, "", |_| vec![]), footer: &footer };
    view::draw(f, theme, f.area(), popup, |room| view::pick_lines(theme, &picks, selected, room));
}

#[cfg(test)]
mod tests {
    use super::super::app::fixtures::{app, on_the_result, opened_on};
    use super::super::ui;
    use super::App;
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
}
