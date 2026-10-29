//! How each column of the grid is coloured, decided once per result and format so a frame only reads it.
use crate::db::model::{ColumnKind, ResultSet, Value};
use crate::format::Format;

/// The most distinct texts a column colours one by one: past it, vtui's six colours repeat so often they no longer
/// tell values apart, and the column is more likely names than a status.
const MOST_REPEATED_TEXTS: usize = 8;

/// What a column's colours depend on beyond each value's kind: its rows, or its format.
/// A value's kind alone colours the rest: `NULL`, a boolean, a negative number, a UUID, JSON.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::tui) enum Colouring {
    Plain,
    /// Few distinct texts, each repeated: one colour per text, the same in every result.
    PerText,
    /// A timestamp read as a date and a time: the time dimmed after the date.
    DateTime,
}

/// The colouring of each column of `result` in its format of `formats`, read on its first `sample` rows.
pub(super) fn colourings(result: &ResultSet, formats: &[Format], sample: usize) -> Vec<Colouring> {
    let columns = result.columns().iter().zip(formats).enumerate();
    columns.map(|(at, (column, &format))| colouring(result, at, &column.kind, format, sample)).collect()
}

fn colouring(result: &ResultSet, at: usize, kind: &ColumnKind, format: Format, sample: usize) -> Colouring {
    let sampled = || result.rows().iter().take(sample).filter_map(|row| row.get(at));
    match (kind, format) {
        (ColumnKind::Timestamp | ColumnKind::TimestampTz, Format::Raw | Format::Local) => Colouring::DateTime,
        (ColumnKind::Text, _) if has_few_repeated_texts(sampled()) => Colouring::PerText,
        _ => Colouring::Plain,
    }
}

/// Whether `values` hold at most [`MOST_REPEATED_TEXTS`] distinct texts, each found twice on average.
fn has_few_repeated_texts<'a>(values: impl Iterator<Item = &'a Value>) -> bool {
    let mut distinct: Vec<&str> = vec![];
    let mut count = 0;
    for value in values {
        let Value::Text(text) = value else { continue };
        count += 1;
        if distinct.contains(&&**text) {
            continue;
        }
        if distinct.len() == MOST_REPEATED_TEXTS {
            return false;
        }
        distinct.push(text);
    }
    !distinct.is_empty() && count >= 2 * distinct.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::column;

    fn texts(texts: &[&str]) -> ResultSet {
        let rows = texts.iter().map(|&text| vec![Value::Text(text.into())]).collect();
        ResultSet::whole(vec![column("status", ColumnKind::Text)], rows)
    }

    fn colouring_of(result: &ResultSet, format: Format) -> Colouring {
        colourings(result, &[format], usize::MAX)[0]
    }

    fn one_of(kind: ColumnKind, format: Format) -> Colouring {
        colouring_of(&ResultSet::whole(vec![column("c", kind)], vec![]), format)
    }

    #[test]
    fn a_text_column_of_few_repeated_values_is_coloured_per_text() {
        assert_eq!(colouring_of(&texts(&["paid", "open", "paid", "open"]), Format::Raw), Colouring::PerText);
    }

    #[test]
    fn a_text_column_whose_values_do_not_repeat_stays_plain() {
        assert_eq!(colouring_of(&texts(&["nina", "noel", "nina"]), Format::Raw), Colouring::Plain);
    }

    #[test]
    fn a_text_column_of_more_distinct_values_than_the_palette_tells_apart_stays_plain() {
        let many: Vec<String> = (0..=MOST_REPEATED_TEXTS).map(|n| format!("status {n}")).collect();
        let twice: Vec<&str> = many.iter().chain(&many).map(String::as_str).collect();
        assert_eq!(colouring_of(&texts(&twice), Format::Raw), Colouring::Plain);
    }

    #[test]
    fn a_text_column_of_nulls_only_stays_plain() {
        let nulls = ResultSet::whole(vec![column("status", ColumnKind::Text)], vec![vec![Value::Null]; 4]);
        assert_eq!(colouring_of(&nulls, Format::Raw), Colouring::Plain);
    }

    #[test]
    fn only_the_sampled_rows_decide_a_text_columns_colouring() {
        let result = texts(&["paid", "paid", "nina", "noel"]);
        assert_eq!(colourings(&result, &[Format::Raw], 2), [Colouring::PerText]);
    }

    #[test]
    fn a_timestamp_splits_its_date_from_its_time_only_when_it_reads_as_a_date_and_a_time() {
        assert_eq!(one_of(ColumnKind::TimestampTz, Format::Local), Colouring::DateTime);
        assert_eq!(one_of(ColumnKind::TimestampTz, Format::Relative), Colouring::Plain);
    }
}
