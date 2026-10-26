use crate::connection::ConnectionName;
use crate::render;
use serde::Serialize;

/// How a check went: fine, worth a look, or broken.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Verdict {
    Ok,
    Warn,
    Fail,
}

/// One line of the report: what was checked, what was found and, unless it is fine, how to fix it.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(super) struct Check {
    pub(super) name: &'static str,
    pub(super) verdict: Verdict,
    pub(super) found: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) fix: Option<String>,
}

/// Every check run on one connection, in the order they ran.
#[derive(Debug, Serialize)]
pub(super) struct Report<'a> {
    pub(super) connection: &'a ConnectionName,
    pub(super) checks: Vec<Check>,
}

impl Check {
    pub(super) fn ok(name: &'static str, found: impl Into<String>) -> Self {
        Self { name, verdict: Verdict::Ok, found: found.into(), fix: None }
    }

    pub(super) fn warn(name: &'static str, found: impl Into<String>, fix: impl Into<String>) -> Self {
        Self { name, verdict: Verdict::Warn, found: found.into(), fix: Some(fix.into()) }
    }

    pub(super) fn fail(name: &'static str, found: impl Into<String>, fix: impl Into<String>) -> Self {
        Self { name, verdict: Verdict::Fail, found: found.into(), fix: Some(fix.into()) }
    }

    fn row(&self) -> Vec<String> {
        let fix = self.fix.as_ref().map_or_else(String::new, |fix| format!(" → {fix}"));
        vec![self.verdict.mark().to_owned(), self.name.to_owned(), format!("{}{fix}", self.found)]
    }
}

impl Verdict {
    fn mark(self) -> &'static str {
        match self {
            Self::Ok => "✓",
            Self::Warn => "!",
            Self::Fail => "✗",
        }
    }
}

impl Report<'_> {
    /// How many checks found something broken.
    pub(super) fn failures(&self) -> usize {
        self.checks.iter().filter(|check| check.verdict == Verdict::Fail).count()
    }

    /// One aligned line per check: its mark, its name, what it found, then the fix after an arrow.
    pub(super) fn text(&self) -> String {
        render::table(&self.checks.iter().map(Check::row).collect::<Vec<_>>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(checks: Vec<Check>) -> String {
        Report { connection: &"shop".parse().unwrap(), checks }.text()
    }

    #[test]
    fn each_check_prints_its_mark_its_name_and_what_it_found_aligned() {
        let text = report(vec![Check::ok("login", "as nina"), Check::ok("timeout", "30s, as production asks")]);
        assert_eq!(text, "✓  login    as nina\n✓  timeout  30s, as production asks\n");
    }

    #[test]
    fn a_check_that_is_not_fine_prints_its_fix_after_an_arrow() {
        let text = report(vec![Check::warn("file mode", "others can read config.toml", "chmod 600 config.toml")]);
        assert_eq!(text, "!  file mode  others can read config.toml → chmod 600 config.toml\n");
    }

    #[test]
    fn a_failed_check_is_marked_with_a_cross_and_only_it_counts_as_a_failure() {
        let checks = vec![Check::ok("login", "as nina"), Check::warn("tls", "off", "add sslmode"), Check::fail("role", "?", "?")];
        let report = Report { connection: &"shop".parse().unwrap(), checks };
        assert_eq!((report.failures(), report.text().lines().last()), (1, Some("✗  role   ? → ?")));
    }

    #[test]
    fn json_names_each_verdict_and_leaves_out_a_fix_that_is_not_needed() {
        let checks = vec![Check::ok("login", "as nina"), Check::warn("tls", "off", "add sslmode")];
        let json = serde_json::to_value(Report { connection: &"shop".parse().unwrap(), checks }).unwrap();
        assert_eq!(json["connection"], "shop");
        assert_eq!((&json["checks"][0]["verdict"], json["checks"][0].get("fix")), (&"ok".into(), None));
        assert_eq!((&json["checks"][1]["verdict"], &json["checks"][1]["fix"]), (&"warn".into(), &"add sslmode".into()));
    }
}
