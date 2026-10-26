//! Snapshots that keep the styles: each line of a buffer as runs of one colour and modifier, so a test sees what
//! the plain text snapshot drops.
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

/// One run per change of style on each row, written `fg/modifiers "text"`; rows are split by `---`.
pub(super) fn runs(buffer: &Buffer) -> String {
    let area = buffer.area;
    let rows = (area.top()..area.bottom()).map(|y| row_runs(buffer, y));
    rows.collect::<Vec<_>>().join("\n---\n")
}

fn row_runs(buffer: &Buffer, y: u16) -> String {
    let mut runs: Vec<(Color, Modifier, String)> = vec![];
    for x in buffer.area.left()..buffer.area.right() {
        let cell = &buffer[(x, y)];
        match runs.last_mut() {
            Some((fg, modifier, text)) if *fg == cell.fg && *modifier == cell.modifier => text.push_str(cell.symbol()),
            _ => runs.push((cell.fg, cell.modifier, cell.symbol().to_owned())),
        }
    }
    runs.iter().map(|(fg, modifier, text)| format!("{fg:?}/{modifier:?} {text:?}")).collect::<Vec<_>>().join("\n")
}
