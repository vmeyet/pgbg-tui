use super::{Column, Value};
use serde::ser::{Serialize, SerializeStruct, Serializer};

/// How much of a statement's rows a result, or a page of it, holds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Extent {
    /// Every row the statement returned.
    #[default]
    All,
    /// The first rows only: the statement returned more than pgbg keeps, or its portal was closed before the last page.
    Cut,
    /// The first rows so far: the rest wait in the portal, a page at a time.
    More,
}

/// One batch of rows fetched from the portal, and how much of the result the rows so far make with it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Page {
    pub(crate) rows: Vec<Vec<Value>>,
    pub(crate) extent: Extent,
}

/// The columns and rows a statement returned, page by page; each row has one value per column.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ResultSet {
    columns: Vec<Column>,
    rows: Vec<Vec<Value>>,
    extent: Extent,
    /// How many pages its rows came in.
    pages: usize,
}

impl ResultSet {
    /// The result of a statement returning `columns`, from its first page.
    pub(crate) fn new(columns: Vec<Column>, first: Page) -> Self {
        Self { columns, rows: first.rows, extent: first.extent, pages: 1 }
    }

    /// A result of every row the statement returned.
    #[cfg(test)]
    pub(crate) fn whole(columns: Vec<Column>, rows: Vec<Vec<Value>>) -> Self {
        Self::new(columns, Page { rows, extent: Extent::All })
    }

    /// The result with the cell at `row` and `column` set to `value`.
    #[cfg(test)]
    pub(crate) fn with_cell(mut self, row: usize, column: usize, value: Value) -> Self {
        self.rows[row][column] = value;
        self
    }

    pub(crate) fn columns(&self) -> &[Column] {
        &self.columns
    }

    pub(crate) fn rows(&self) -> &[Vec<Value>] {
        &self.rows
    }

    pub(crate) fn extent(&self) -> Extent {
        self.extent
    }

    /// Whether its rows came in more than one page.
    pub(crate) fn is_paged(&self) -> bool {
        self.pages > 1
    }

    /// Whether it holds every row the statement returned: not cut, and no more waiting in the portal.
    pub(crate) fn is_whole(&self) -> bool {
        self.extent == Extent::All
    }

    /// Adds the next page's rows; the result then holds as much as that page says. Only a result waiting on more pages
    /// takes one: a page pushed on a whole or cut result is dropped.
    pub(crate) fn push(&mut self, page: Page) {
        debug_assert_eq!(self.extent, Extent::More, "a page pushed on a result that waits on none");
        if self.extent != Extent::More {
            return;
        }
        self.rows.extend(page.rows);
        self.extent = page.extent;
        self.pages += 1;
    }

    /// Stops waiting on the portal, closed with its session: a result waiting on more pages becomes cut.
    pub(crate) fn cut(&mut self) {
        if self.extent == Extent::More {
            self.extent = Extent::Cut;
        }
    }
}

/// `{"columns", "rows", "is_cut"}`: a script reads a result that waits on more pages as cut, since it gets no more.
impl Serialize for ResultSet {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut json = serializer.serialize_struct("ResultSet", 3)?;
        json.serialize_field("columns", &self.columns)?;
        json.serialize_field("rows", &self.rows)?;
        json.serialize_field("is_cut", &!self.is_whole())?;
        json.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::model::ColumnKind;
    use crate::fixtures::column;

    fn ids(ids: &[i64]) -> Vec<Vec<Value>> {
        ids.iter().map(|&id| vec![Value::Int(id)]).collect()
    }

    fn first_page_of_more() -> ResultSet {
        ResultSet::new(vec![column("id", ColumnKind::Int)], Page { rows: ids(&[1, 2]), extent: Extent::More })
    }

    #[test]
    fn a_pushed_page_adds_its_rows_after_the_others() {
        let mut result = first_page_of_more();
        result.push(Page { rows: ids(&[3]), extent: Extent::More });
        assert_eq!(result.rows(), ids(&[1, 2, 3]));
    }

    #[test]
    fn the_last_page_pushed_makes_the_result_whole() {
        let mut result = first_page_of_more();
        result.push(Page { rows: ids(&[3]), extent: Extent::All });
        assert_eq!(result.extent(), Extent::All);
    }

    #[test]
    fn a_result_with_a_page_pushed_is_paged() {
        let mut result = first_page_of_more();
        assert!(!result.is_paged());
        result.push(Page { rows: ids(&[3]), extent: Extent::All });
        assert!(result.is_paged());
    }

    #[test]
    fn a_cut_result_says_so_in_json() {
        let result = ResultSet::new(vec![], Page { rows: vec![], extent: Extent::Cut });
        assert_eq!(serde_json::to_value(&result).unwrap()["is_cut"], true);
    }

    #[test]
    fn a_result_waiting_on_more_pages_is_cut_in_json() {
        assert_eq!(serde_json::to_value(first_page_of_more()).unwrap()["is_cut"], true);
    }

    #[test]
    fn a_whole_result_in_json_keeps_the_shape_scripts_read() {
        let json = serde_json::to_value(ResultSet::whole(vec![column("id", ColumnKind::Int)], ids(&[7]))).unwrap();
        let shape = serde_json::json!({"columns": [{"name": "id", "kind": "int", "type": ""}], "rows": [[7]], "is_cut": false});
        assert_eq!(json, shape);
    }
}
