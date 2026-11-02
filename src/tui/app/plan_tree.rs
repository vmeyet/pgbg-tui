//! A plan as a tree in the result pane, each node folding as the JSON tree's values do.
use super::folds::{Folds, Mark, Span};
use super::sheet::Run;
use super::{App, View};
use crate::act::Act;
use crate::db::model::ResultSet;
use crate::plan::{Node, Plan};

/// A line as the pane draws it: its node, its mark and whether it is the costliest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::tui) struct PlanLine<'p> {
    pub(in crate::tui) node: &'p Node,
    pub(in crate::tui) mark: Mark,
    pub(in crate::tui) is_costliest: bool,
}

/// A plan, its folded nodes and the cursor's line.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::tui) struct PlanTree {
    plan: Plan,
    pub(in crate::tui) folds: Folds,
}

impl PlanTree {
    /// The tree of the plan `result` holds, when it holds one.
    pub(super) fn of(result: &ResultSet) -> Option<Self> {
        let plan = Plan::of(result)?;
        let folds = Folds::new(plan.nodes.iter().map(|node| Span { parent: node.parent, end: node.end }).collect());
        Some(Self { plan, folds })
    }

    /// The `count` lines from `first` on, built for a frame that draws only those.
    pub(in crate::tui) fn lines_in(&self, first: usize, count: usize) -> Vec<PlanLine<'_>> {
        let line = |index: usize| PlanLine {
            node: &self.plan.nodes[index],
            mark: self.folds.mark(index),
            is_costliest: self.plan.costliest == Some(index),
        };
        self.folds.shown_in(first, count).map(line).collect()
    }
}

impl App {
    /// Takes `act` on the plan on screen; false leaves it to what works anywhere.
    pub(super) fn plan_act(&mut self, act: Act) -> bool {
        let Run::Done { view: View::Plan(tree), .. } = &mut self.sheet_mut().run else { return false };
        match act {
            Act::Down => tree.folds.step(1),
            Act::Up => tree.folds.step(-1),
            Act::Open => tree.folds.toggle(),
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::{enter, on_the_result, press, to_shop, typed, view};
    use super::super::{Incoming, RunId};
    use super::*;
    use crate::act::Pane;
    use crate::fixtures::{ANALYZED_PLAN, ESTIMATED_PLAN, explain_result, orders};
    use crossterm::event::KeyCode;
    use std::time::Duration;

    fn on_the_plan() -> App {
        on_the_result(explain_result(ESTIMATED_PLAN))
    }

    fn tree(app: &App) -> &PlanTree {
        let View::Plan(tree) = view(app) else { panic!("no plan") };
        tree
    }

    fn kinds(app: &App) -> Vec<&str> {
        tree(app).lines_in(0, 9).iter().map(|line| line.node.kind.as_str()).collect()
    }

    #[test]
    fn a_plan_the_user_explained_opens_as_a_tree() {
        let mut app = typed("EXPLAIN (FORMAT JSON) SELECT 1");
        enter(&mut app);
        app.apply(to_shop(Incoming::Result { run: RunId::FIRST, result: explain_result(ANALYZED_PLAN), took: Duration::ZERO }));
        assert!(matches!(view(&app), View::Plan(_)));
    }

    #[test]
    fn an_explain_of_an_unexpected_shape_stays_a_grid() {
        assert_eq!(view(&on_the_result(explain_result(r#"{"Plan":7}"#))), &View::Grid);
    }

    #[test]
    fn the_result_pane_takes_the_plans_keys_only_while_a_plan_shows() {
        assert_eq!((on_the_plan().pane(), on_the_result(orders()).pane()), (Pane::Plan, Pane::Result));
    }

    #[test]
    fn j_and_k_move_the_cursor_within_the_lines() {
        let mut app = on_the_plan();
        (0..9).for_each(|_| press(&mut app, KeyCode::Char('j')));
        assert_eq!(tree(&app).folds.at, 3);
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(tree(&app).folds.at, 2);
    }

    #[test]
    fn enter_folds_a_node_and_again_brings_its_children_back() {
        let mut app = on_the_plan();
        (0..2).for_each(|_| press(&mut app, KeyCode::Char('j')));
        press(&mut app, KeyCode::Enter);
        assert_eq!(kinds(&app), ["Hash Join", "Seq Scan", "Hash"]);
        press(&mut app, KeyCode::Enter);
        assert_eq!(kinds(&app).len(), 4);
    }

    #[test]
    fn enter_on_a_node_without_children_folds_its_parent_and_moves_there() {
        let mut app = on_the_plan();
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Enter);
        assert_eq!((kinds(&app), tree(&app).folds.at), (vec!["Hash Join"], 0));
    }

    #[test]
    fn v_shows_the_plan_as_a_grid_then_as_a_tree_again() {
        let mut app = on_the_plan();
        press(&mut app, KeyCode::Char('v'));
        assert_eq!(view(&app), &View::Grid);
        press(&mut app, KeyCode::Char('v'));
        assert!(matches!(view(&app), View::Plan(_)));
    }
}
