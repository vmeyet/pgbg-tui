//! The `[tui]` table: how the TUI looks, keyed as in the sibling TUIs that share vtui.
use serde::Deserialize;
use vtui::theme::Theme;

/// The `[tui]` table.
#[derive(Default, Deserialize)]
pub(super) struct TuiTable {
    #[serde(default)]
    theme: Option<String>,
}

/// The palette the table names, and a warning when it names none vtui knows.
#[derive(Debug)]
pub(super) struct ReadTui {
    pub(super) theme: Theme,
    pub(super) warnings: Vec<String>,
}

/// The palette `[tui] theme` names; an unknown name falls back to the default one with a warning, so a typo never
/// stops pgbg.
pub(super) fn read(table: TuiTable) -> ReadTui {
    let Some(name) = table.theme else { return ReadTui { theme: Theme::default(), warnings: vec![] } };
    match Theme::named(&name) {
        Some(theme) => ReadTui { theme, warnings: vec![] },
        None => ReadTui { theme: Theme::default(), warnings: vec![unknown_theme(&name)] },
    }
}

fn unknown_theme(name: &str) -> String {
    format!("`tui.theme = \"{name}\"` is not a theme; the default one is used — try {}", Theme::NAMES.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_named(name: &str) -> ReadTui {
        read(TuiTable { theme: Some(name.to_owned()) })
    }

    #[test]
    fn no_theme_is_the_default_palette_without_a_warning() {
        let ReadTui { theme, warnings } = read(TuiTable::default());
        assert_eq!((theme.name, warnings), ("default", vec![]));
    }

    #[test]
    fn a_known_name_picks_its_palette() {
        assert_eq!(read_named("dracula").theme.name, "dracula");
    }

    #[test]
    fn case_spaces_and_dashes_do_not_matter_in_a_name() {
        assert_eq!(read_named("Tokyo Night").theme.name, "tokyonight");
        assert_eq!(read_named("rose-pine").theme.name, "rosepine");
    }

    #[test]
    fn an_unknown_name_warns_with_the_names_known_and_falls_back_to_the_default() {
        let ReadTui { theme, warnings } = read_named("solarized");
        assert_eq!(theme.name, "default");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with("`tui.theme = \"solarized\"` is not a theme"), "{warnings:?}");
        assert!(Theme::NAMES.iter().all(|name| warnings[0].contains(name)), "{warnings:?}");
    }
}
