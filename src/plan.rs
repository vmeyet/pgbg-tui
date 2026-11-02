//! The plan an `EXPLAIN (FORMAT JSON)` result holds, pure: its nodes in the order they are written, what each one costs
//! on its own, and which one costs most.
//! A field pgbg does not know is skipped; a result of any other shape gives no plan, so it stays a plain result.
use crate::db::model::{ColumnKind, ResultSet, Value};
use serde::Deserialize;

/// The name the server gives the one column of an `EXPLAIN`.
const PLAN_COLUMN: &str = "QUERY PLAN";
/// How many times over an estimate must miss the rows counted before it is worth flagging.
const FAR_OFF_TIMES: f64 = 10.0;

/// The one entry of the JSON array; its timings beside the plan are not read.
#[derive(Deserialize)]
struct Explained {
    #[serde(rename = "Plan")]
    plan: Written,
}

/// A node as the server writes it; `COSTS OFF` leaves out the estimates and `TIMING OFF` the times.
#[derive(Deserialize)]
struct Written {
    #[serde(rename = "Node Type")]
    kind: String,
    #[serde(rename = "Relation Name")]
    relation: Option<String>,
    #[serde(rename = "Plan Rows")]
    rows: Option<f64>,
    #[serde(rename = "Total Cost")]
    cost: Option<f64>,
    #[serde(rename = "Actual Rows")]
    actual_rows: Option<f64>,
    #[serde(rename = "Actual Loops")]
    actual_loops: Option<f64>,
    #[serde(rename = "Actual Total Time")]
    actual_time_ms: Option<f64>,
    #[serde(rename = "Plans", default)]
    children: Vec<Self>,
}

impl Written {
    /// The time it took over all its loops, as the server gives the time of one loop.
    fn time_of_all_loops_ms(&self) -> Option<f64> {
        Some(self.actual_time_ms? * self.actual_loops?)
    }

    /// Its `figure` less that of its children: never below zero, as a `Limit` stops a child early, and the loops of a
    /// parallel child add up the time of every worker.
    fn alone(&self, figure: impl Fn(&Self) -> Option<f64>) -> Option<f64> {
        let children: f64 = self.children.iter().filter_map(&figure).sum();
        Some((figure(self)? - children).max(0.0))
    }
}

/// What a node did when the statement ran.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Actual {
    /// The rows of one loop on average, as `rows` estimates them.
    pub(crate) rows: f64,
    pub(crate) loops: f64,
    /// Its time over all its loops, its children's left out.
    pub(crate) self_time_ms: Option<f64>,
}

/// One step of the plan.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Node {
    pub(crate) parent: Option<usize>,
    pub(crate) depth: usize,
    /// The first node past its own children, at any depth.
    pub(crate) end: usize,
    /// As the server names it: `Seq Scan`, `Hash Join`.
    pub(crate) kind: String,
    pub(crate) relation: Option<String>,
    /// The rows it is estimated to return.
    pub(crate) rows: Option<f64>,
    /// Its estimated cost, its children's included.
    pub(crate) cost: Option<f64>,
    self_cost: Option<f64>,
    pub(crate) actual: Option<Actual>,
}

impl Node {
    /// How many times over the estimate missed the rows counted, once that is far off; a node that never ran says nothing.
    pub(crate) fn estimate_off_by(&self) -> Option<f64> {
        let (estimate, actual) = (self.rows?, self.actual.as_ref().filter(|actual| actual.loops > 0.0)?);
        let (estimate, counted) = (estimate.max(1.0), actual.rows.max(1.0));
        Some(estimate.max(counted) / estimate.min(counted)).filter(|&times| times >= FAR_OFF_TIMES)
    }
}

/// The nodes of a plan, each parent before its children.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Plan {
    pub(crate) nodes: Vec<Node>,
    /// The node that took most time on its own; by estimated cost when nothing timed it.
    pub(crate) costliest: Option<usize>,
}

impl Plan {
    /// The plan `result` holds, when it is what an `EXPLAIN (FORMAT JSON)` returns and its JSON reads as one.
    pub(crate) fn of(result: &ResultSet) -> Option<Self> {
        let [row] = result.rows() else { return None };
        match row.as_slice() {
            [Value::Json(json)] if holds_a_plan(result) => Self::read(json.as_str()),
            _ => None,
        }
    }

    /// The plan in `json`; none when it is not the one plan the server writes for one statement.
    fn read(json: &str) -> Option<Self> {
        let [Explained { plan }]: [Explained; 1] = serde_json::from_str(json).ok()?;
        let nodes = flattened(plan, None, 0, 0);
        let costliest = most(&nodes, |node| node.actual.as_ref()?.self_time_ms).or_else(|| most(&nodes, |node| node.self_cost));
        Some(Self { nodes, costliest })
    }
}

/// Whether the columns of `result` are those of an `EXPLAIN (FORMAT JSON)`, whose row may not read as a plan still.
pub(crate) fn holds_a_plan(result: &ResultSet) -> bool {
    matches!(result.columns(), [column] if column.name.as_str() == PLAN_COLUMN && column.kind == ColumnKind::Json)
}

/// `written` and its children in order, numbered from `at`.
fn flattened(written: Written, parent: Option<usize>, depth: usize, at: usize) -> Vec<Node> {
    let self_cost = written.alone(|node| node.cost);
    let self_time_ms = written.alone(Written::time_of_all_loops_ms);
    let actual = written.actual_rows.zip(written.actual_loops).map(|(rows, loops)| Actual { rows, loops, self_time_ms });
    let Written { kind, relation, rows, cost, children, .. } = written;
    let mut below = vec![];
    for child in children {
        let placed = flattened(child, Some(at), depth + 1, at + 1 + below.len());
        below.extend(placed);
    }
    let end = at + 1 + below.len();
    std::iter::once(Node { parent, depth, end, kind, relation, rows, cost, self_cost, actual }).chain(below).collect()
}

/// The first of the nodes whose `figure` is the highest; none when no node has one above zero.
fn most(nodes: &[Node], figure: impl Fn(&Node) -> Option<f64>) -> Option<usize> {
    let figures = nodes.iter().enumerate().filter_map(|(at, node)| Some((at, figure(node)?))).filter(|&(_, figure)| figure > 0.0);
    figures.reduce(|highest, next| if next.1 > highest.1 { next } else { highest }).map(|(at, _)| at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{ANALYZED_PLAN, ESTIMATED_PLAN, column, explain_result, orders};

    fn plan(json: &str) -> Plan {
        Plan::read(json).unwrap()
    }

    #[test]
    fn nodes_come_in_the_order_written_with_their_depth_and_parent() {
        let nodes = plan(ESTIMATED_PLAN).nodes;
        let read: Vec<_> = nodes.iter().map(|node| (node.kind.as_str(), node.depth, node.parent, node.end)).collect();
        assert_eq!(read, [("Hash Join", 0, None, 4), ("Seq Scan", 1, Some(0), 2), ("Hash", 1, Some(0), 4), ("Seq Scan", 2, Some(2), 4)]);
    }

    #[test]
    fn a_node_reads_its_relation_rows_and_cost() {
        let node = &plan(ESTIMATED_PLAN).nodes[1];
        assert_eq!((node.relation.as_deref(), node.rows, node.cost), (Some("orders"), Some(500.0), Some(70.0)));
    }

    #[test]
    fn without_times_the_costliest_node_is_the_one_with_the_highest_cost_of_its_own() {
        assert_eq!(plan(ESTIMATED_PLAN).costliest, Some(1));
    }

    #[test]
    fn with_times_the_costliest_node_is_the_one_that_took_most_time_of_its_own() {
        assert_eq!(plan(ANALYZED_PLAN).costliest, Some(2));
    }

    #[test]
    fn the_self_time_of_a_node_counts_all_its_loops_and_leaves_its_children_out() {
        let nodes = plan(ANALYZED_PLAN).nodes;
        let self_time = |at: usize| nodes[at].actual.as_ref().unwrap().self_time_ms;
        assert_eq!((self_time(0), self_time(2)), (Some(4.0), Some(20.0)));
    }

    #[test]
    fn a_child_costing_more_than_its_parent_leaves_the_parent_at_zero() {
        let json = r#"[{"Plan":{"Node Type":"Limit","Total Cost":1.0,"Plans":[{"Node Type":"Seq Scan","Total Cost":90.0}]}}]"#;
        assert_eq!(plan(json).nodes[0].self_cost, Some(0.0));
    }

    #[test]
    fn a_plan_without_costs_or_times_reads_and_has_no_costliest_node() {
        let read = plan(r#"[{"Plan":{"Node Type":"Seq Scan","Relation Name":"orders"}}]"#);
        assert_eq!((read.nodes[0].cost, read.nodes[0].actual.is_none(), read.costliest), (None, true, None));
    }

    #[test]
    fn rows_counted_without_timing_still_read() {
        let read = plan(r#"[{"Plan":{"Node Type":"Seq Scan","Actual Rows":3.5,"Actual Loops":2}}]"#);
        assert_eq!(read.nodes[0].actual, Some(Actual { rows: 3.5, loops: 2.0, self_time_ms: None }));
    }

    #[test]
    fn an_estimate_ten_times_too_low_is_far_off() {
        assert_eq!(plan(ANALYZED_PLAN).nodes[0].estimate_off_by(), Some(80.0));
    }

    #[test]
    fn an_estimate_ten_times_too_high_is_far_off() {
        let read = plan(r#"[{"Plan":{"Node Type":"Seq Scan","Plan Rows":500,"Actual Rows":5,"Actual Loops":1}}]"#);
        assert_eq!(read.nodes[0].estimate_off_by(), Some(100.0));
    }

    #[test]
    fn an_estimate_close_to_the_rows_counted_is_not_far_off() {
        assert_eq!(plan(ANALYZED_PLAN).nodes[1].estimate_off_by(), None);
    }

    #[test]
    fn a_node_that_never_ran_says_nothing_of_its_estimate() {
        let read = plan(r#"[{"Plan":{"Node Type":"Seq Scan","Plan Rows":500,"Actual Rows":0,"Actual Loops":0}}]"#);
        assert_eq!(read.nodes[0].estimate_off_by(), None);
    }

    #[test]
    fn an_explain_result_gives_its_plan() {
        assert_eq!(Plan::of(&explain_result(ESTIMATED_PLAN)), Some(plan(ESTIMATED_PLAN)));
    }

    #[test]
    fn a_result_of_other_columns_gives_no_plan() {
        assert_eq!(Plan::of(&orders()), None);
    }

    #[test]
    fn json_of_another_shape_gives_no_plan() {
        let two_plans = r#"[{"Plan":{"Node Type":"Result"}},{"Plan":{"Node Type":"Result"}}]"#;
        for json in [r#"{"Plan":{}}"#, r#"[{"Plan":{"Total Cost":1}}]"#, r#"[{"Plan":{"Node Type":7}}]"#, "[]", "1", two_plans] {
            assert_eq!(Plan::of(&explain_result(json)), None, "{json}");
        }
    }

    #[test]
    fn a_plan_nested_past_what_the_parser_allows_gives_no_plan() {
        let (open, close) = (r#"{"Node Type":"Append","Plans":["#, "]}");
        let json = format!(r#"[{{"Plan":{}{{"Node Type":"Result"}}{}}}]"#, open.repeat(100), close.repeat(100));
        assert_eq!(Plan::read(&json), None);
    }

    #[test]
    fn the_text_format_of_an_explain_gives_no_plan() {
        let result = ResultSet::whole(vec![column(PLAN_COLUMN, ColumnKind::Text)], vec![vec![Value::Text("Seq Scan".into())]]);
        assert_eq!((holds_a_plan(&result), Plan::of(&result)), (false, None));
    }
}
