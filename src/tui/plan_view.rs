//! A plan in the result pane: one node a line, indented, `▾` before an open node and `▸` before a folded one, the
//! costliest node lit and an estimate far off the rows counted flagged, both before the figures a narrow pane cuts.
use super::app::{PlanLine, PlanTree};
use super::json_tree_view::{lead, lit};
use crate::format;
use crate::plan::{Actual, Node};
use crate::render;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use vtui::theme::Theme;

const FIGURES_APART: &str = " · ";

/// The tree in `area`, scrolled so the cursor's line shows; the cursor shows while the pane has the keys.
pub(super) fn draw(f: &mut Frame<'_>, tree: &PlanTree, theme: &Theme, focused: bool, area: Rect) {
    let height = usize::from(area.height);
    let first = tree.folds.first_line_in(height);
    let window = tree.lines_in(first, height);
    let lines: Vec<Line<'static>> =
        window.iter().zip(first..).map(|(line, at)| drawn(theme, line, focused && at == tree.folds.at)).collect();
    f.render_widget(Paragraph::new(lines), area);
}

fn drawn(theme: &Theme, line: &PlanLine<'_>, is_cursor: bool) -> Line<'static> {
    let node = line.node;
    let costliest = Style::default().fg(theme.danger).add_modifier(Modifier::BOLD);
    let spans = [
        Some(lead(theme, node.depth, line.mark)),
        Some(Span::styled(title(node), if line.is_costliest { costliest } else { Style::default() })),
        line.is_costliest.then(|| Span::styled("  ← costliest", costliest)),
        node.estimate_off_by().map(|times| Span::styled(format!("  est off ×{times:.0}"), Style::default().fg(theme.warn))),
        Some(Span::styled(figures(node), Style::default().fg(theme.muted))),
    ];
    lit(Line::from(spans.into_iter().flatten().collect::<Vec<_>>()), is_cursor)
}

/// `Seq Scan on orders`, as the server names them, escaped.
fn title(node: &Node) -> String {
    let kind = render::printable(&node.kind);
    match &node.relation {
        Some(relation) => format!("{kind} on {}", render::printable(relation)),
        None => kind.into_owned(),
    }
}

/// `  est 500 rows · cost 70.00  actual 980 rows × 1 · self 1.2 ms`, each part when the plan gives it.
fn figures(node: &Node) -> String {
    let estimate = [node.rows.map(|rows| format!("est {} rows", count(rows))), node.cost.map(|cost| format!("cost {cost:.2}"))];
    let actual = node.actual.as_ref().map(actual_figures).into_iter().flatten();
    let parts = [estimate.into_iter().flatten().collect::<Vec<_>>(), actual.collect()];
    parts.iter().filter(|part| !part.is_empty()).map(|part| format!("  {}", part.join(FIGURES_APART))).collect()
}

fn actual_figures(actual: &Actual) -> Vec<String> {
    let rows = format!("actual {} rows × {}", count(actual.rows), count(actual.loops));
    std::iter::once(rows).chain(actual.self_time_ms.map(|ms| format!("self {ms:.1} ms"))).collect()
}

/// A count grouped as the grid groups a number; the server's average over loops keeps its decimals.
fn count(figure: f64) -> String {
    format::grouped_decimal(&figure.to_string()).unwrap_or_else(|| figure.to_string())
}

#[cfg(test)]
mod tests {
    use super::super::app::App;
    use super::super::app::fixtures::{on_the_result, press};
    use super::super::{styled, ui};
    use super::*;
    use crate::fixtures::{ANALYZED_PLAN, ESTIMATED_PLAN, explain_result};
    use crossterm::event::KeyCode;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn on_the_plan(json: &str) -> App {
        on_the_result(explain_result(json))
    }

    fn render(app: &App) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(110, 16)).unwrap();
        terminal.draw(|f| _ = ui::draw(f, app)).unwrap();
        terminal
    }

    #[test]
    fn a_plan_of_estimates_lights_the_node_with_the_highest_cost_of_its_own() {
        let app = on_the_plan(ESTIMATED_PLAN);
        insta::assert_snapshot!("plan_estimated_styled", styled::runs(render(&app).backend().buffer()));
    }

    #[test]
    fn an_analyzed_plan_shows_what_ran_and_flags_an_estimate_far_off() {
        let app = on_the_plan(ANALYZED_PLAN);
        insta::assert_snapshot!("plan_analyzed_styled", styled::runs(render(&app).backend().buffer()));
    }

    #[test]
    fn a_folded_node_hides_its_children() {
        let mut app = on_the_plan(ESTIMATED_PLAN);
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Enter);
        insta::assert_snapshot!("plan_folded", render(&app).backend().to_string());
    }

    #[test]
    fn a_plan_without_costs_shows_its_nodes_alone() {
        let app = on_the_plan(r#"[{"Plan":{"Node Type":"Seq Scan","Relation Name":"orders"}}]"#);
        insta::assert_snapshot!("plan_without_costs", render(&app).backend().to_string());
    }

    #[test]
    fn the_cursor_shows_only_while_the_pane_has_the_keys() {
        let mut app = on_the_plan(ESTIMATED_PLAN);
        press(&mut app, KeyCode::Tab);
        let unlit = styled::runs(render(&app).backend().buffer());
        assert!(unlit.contains("\"▾ \"") && !unlit.contains("REVERSED/"), "{unlit}");
    }

    #[test]
    fn a_control_character_in_a_relation_is_escaped() {
        let app = on_the_plan("[{\"Plan\":{\"Node Type\":\"Seq Scan\",\"Relation Name\":\"ord\\u001bers\"}}]");
        assert!(render(&app).backend().to_string().contains("ord\\u{1b}ers"));
    }

    #[test]
    fn a_count_keeps_the_decimals_of_an_average_and_groups_a_whole_number() {
        assert_eq!([count(1200.0), count(3.5)], ["1 200", "3.5"]);
    }
}
