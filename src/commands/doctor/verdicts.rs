use super::report::Check;
use crate::connection::{Connection, TimeoutChoice, Tls, TunnelSpec, WritePolicy};
use crate::db::model::{LocalPort, Named, ReadOnlyGaps, RoleFlag, RolePowers, RoleRights, ServerFacts};
use std::path::Path;

const PRIVATE_CA: &str = "sslrootcert";
const OTHERS_READ_OR_WRITE: u32 = 0o077;
const READ_ONLY_ROLE: &str = "use a read-only role: the role is the only real guarantee";
/// What any role can do to the sessions of its own role, `pg_signal_backend` or not.
const OWN_SESSIONS: &str = "; it can still cancel or end sessions of its own role, as any role can";

pub(super) fn config_refused(err: &anyhow::Error) -> Check {
    Check::fail("config", format!("{err:#}"), "fix `config.toml` as it says; `pgbg connections` lists what loads")
}

pub(super) fn config(connection: &Connection) -> Check {
    let password = connection.password().map_or_else(|| "no password".to_owned(), |source| format!("password from {}", source.kind()));
    Check::ok("config", format!("`{}` loads, {password}", connection.name()))
}

/// `mode` holds the permission bits of the file at `path`.
pub(super) fn file_mode(path: &Path, mode: u32) -> Check {
    let found = format!("{} is {:04o}", path.display(), mode & 0o777);
    if mode & OTHERS_READ_OR_WRITE == 0 {
        return Check::ok("file mode", found);
    }
    Check::warn("file mode", format!("{found}: other users can read it"), format!("chmod 600 {}", path.display()))
}

pub(super) fn tls(connection: &Connection) -> Check {
    let target = connection.target();
    let asks_private_ca = target.has_query_key(PRIVATE_CA);
    if asks_private_ca {
        return Check::fail(
            "tls",
            "a private CA (`sslrootcert`) is not supported yet",
            "drop `sslrootcert` from `url`: pgbg trusts the public CAs only",
        );
    }
    let why = target.ssl_mode().map_or_else(|| "`tls = \"on\"`".to_owned(), |mode| format!("`sslmode={mode}`"));
    match connection.tls() {
        Tls::On => Check::ok(
            "tls",
            format!(
                "on, as {why} asks; the certificate is checked as `verify-full` does, against the public CAs (a private CA is not supported yet)"
            ),
        ),
        Tls::Off if target.is_local() && connection.tunnel().is_none() => Check::ok("tls", "off: the host is this machine"),
        Tls::Off => Check::warn(
            "tls",
            "off, as `tls = \"off\"` asks: the password and the rows travel in clear",
            "add `sslmode=require` to `url` and drop `tls = \"off\"`",
        ),
    }
}

pub(super) fn tunnel_failed(err: &anyhow::Error) -> Check {
    Check::fail("tunnel", format!("{err:#}"), "run the tunnel's command by hand to see why")
}

pub(super) fn tunnel_started(spec: &TunnelSpec, port: LocalPort) -> Check {
    let program = |argv: &[String]| argv.first().map_or_else(String::new, |program| format!("`{program}`"));
    let before = spec.before().map_or_else(String::new, |before| format!("`before` ({}) ran, then ", program(before)));
    Check::ok("tunnel", format!("{before}{} answers on :{port}", program(spec.command())))
}

pub(super) fn tunnel_stopped(port: LocalPort, is_port_free: bool) -> Check {
    if is_port_free {
        return Check::ok("tunnel stop", format!("stopped, :{port} is free again"));
    }
    Check::warn(
        "tunnel stop",
        format!(":{port} still answers once the tunnel stopped: another process holds it, such as an ssh ControlMaster"),
        "add `-o ControlPath=none` to an ssh tunnel, so pgbg owns all of it",
    )
}

pub(super) fn login_failed(err: &anyhow::Error) -> Check {
    Check::fail("login", format!("{err:#}"), "check the user and the password, and that the server lets this machine in")
}

pub(super) fn not_read(name: &'static str, err: &anyhow::Error) -> Check {
    Check::warn(name, format!("could not read it: {err:#}"), "run `SELECT 1` with `pgbg run` to see why reads fail")
}

pub(super) fn server(facts: &ServerFacts) -> [Check; 2] {
    [Check::ok("login", format!("as `{}`", facts.user)), Check::ok("server", format!("PostgreSQL {}", facts.version))]
}

/// Fine when the session runs with the timeout pgbg set: the connection's own, else the stricter of its tier's and the server's.
pub(super) fn timeout(connection: &Connection, facts: &ServerFacts) -> Check {
    let choice = connection.statement_timeout();
    let set = choice.in_effect(facts.server_statement_timeout);
    let in_effect = facts.statement_timeout;
    if in_effect != set {
        let found = format!("{in_effect} in effect, not the {set} pgbg set");
        return Check::warn(
            "timeout",
            found,
            "a pooler in transaction mode drops `SET`: connect past it, or set `statement_timeout` on the role",
        );
    }
    let tier = connection.tier();
    let why = match choice {
        TimeoutChoice::Own(_) => "the connection's own".to_owned(),
        TimeoutChoice::Tier(tier_timeout) if tier_timeout == set => format!("the {tier} tier's"),
        TimeoutChoice::Tier(tier_timeout) => format!("the server's, stricter than the {tier} tier's {tier_timeout}"),
    };
    Check::ok("timeout", format!("{in_effect}, {why}"))
}

fn flag_power(flag: RoleFlag) -> &'static str {
    match flag {
        RoleFlag::Superuser => "is a superuser",
        RoleFlag::CreateRole => "creates roles",
        RoleFlag::CreateDatabase => "creates databases",
        RoleFlag::BypassRls => "bypasses row security",
        RoleFlag::WriteAllData => "writes every table",
        RoleFlag::CreateSchema => "creates schemas",
    }
}

/// `"writes 14 tables: public.customers, public.orders and 12 more"`, or nothing for none.
fn named_power(verb: &str, kind: &str, named: &Named) -> Option<String> {
    let count = named.count;
    let unnamed = count.saturating_sub(named.first.len() as u64);
    let more = if unnamed > 0 { format!(" and {unnamed} more") } else { String::new() };
    let plural = if count == 1 { "" } else { "s" };
    (count > 0).then(|| format!("{verb} {count} {kind}{plural}: {}{more}", named.first.join(", ")))
}

/// What `powers` lets its role write, named: `` `nina` creates databases, writes 1 table: public.orders ``.
fn role_powers(powers: &RolePowers) -> String {
    let flags = powers.flags.iter().map(|flag| flag_power(*flag).to_owned());
    let named = [
        named_power("creates objects in", "schema", &powers.schemas),
        named_power("writes", "table", &powers.tables),
        named_power("advances", "sequence", &powers.sequences),
    ];
    let held: Vec<String> = flags.chain(named.into_iter().flatten()).collect();
    format!("`{}` {}", powers.role, held.join(", "))
}

pub(super) fn role(writes: WritePolicy, rights: &RoleRights) -> Check {
    if rights.roles.is_empty() {
        return Check::ok("role", "read only: it can write no table");
    }
    let powers: Vec<String> = rights.roles.iter().map(role_powers).collect();
    let found = format!("not read only: {}", powers.join("; "));
    match writes {
        WritePolicy::Allowed => Check::ok("role", format!("{found}; this connection allows writes")),
        WritePolicy::ReadOnly => Check::warn("role", found, READ_ONLY_ROLE),
    }
}

/// `"call `dblink` and `dblink_exec`"`, or nothing for none.
fn listed(verb: &str, names: &[String]) -> Option<String> {
    let quoted: Vec<String> = names.iter().map(|name| format!("`{name}`")).collect();
    let joined = match quoted.as_slice() {
        [] => return None,
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    Some(format!("{verb} {joined}"))
}

pub(super) fn beyond_read_only(writes: WritePolicy, gaps: &ReadOnlyGaps) -> Check {
    let signals = gaps.signals_sessions.then(|| "cancel or end other sessions (`pg_signal_backend`)".to_owned());
    let dblink = listed("call", &gaps.dblink_functions).map(|call| format!("{call}, which opens a session of its own that may write"));
    let settings = listed("set", &gaps.granted_settings);
    let ways: Vec<String> = [signals, dblink, settings].into_iter().flatten().collect();
    let own_sessions = if gaps.signals_sessions { "" } else { OWN_SESSIONS };
    let found = match ways.as_slice() {
        [] => return Check::ok("beyond read only", format!("nothing: a read-only transaction holds the role{own_sessions}")),
        [one] => format!("it can {one}{own_sessions}"),
        [rest @ .., last] => format!("it can {}, and {last}{own_sessions}", rest.join(", ")),
    };
    match writes {
        WritePolicy::Allowed => Check::ok("beyond read only", format!("{found}; this connection allows writes")),
        WritePolicy::ReadOnly => {
            Check::warn("beyond read only", found, "revoke them from the role: a read-only transaction does not stop them")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::doctor::report::Verdict;
    use crate::connection::StatementTimeout;
    use crate::fixtures;

    fn connection(extra: &str) -> Connection {
        fixtures::named("prod", &format!("tier = \"production\"\nurl = \"postgres://nina@db.example.com/shop?sslmode=require\"\n{extra}"))
    }

    fn local(url: &str, extra: &str) -> Connection {
        fixtures::named("local", &format!("tier = \"local\"\nurl = \"{url}\"\n{extra}"))
    }

    #[test]
    fn the_config_check_names_the_password_source_and_never_the_password() {
        assert_eq!(config(&connection("password = { env = \"PROD_PGPASSWORD\" }")).found, "`prod` loads, password from env");
        assert_eq!(config(&connection("")).found, "`prod` loads, no password");
    }

    #[test]
    fn a_config_only_its_owner_reads_is_fine() {
        assert_eq!(file_mode(Path::new("config.toml"), 0o100600), Check::ok("file mode", "config.toml is 0600"));
    }

    #[test]
    fn a_config_others_can_read_is_a_warning_with_the_chmod_that_fixes_it() {
        let check = file_mode(Path::new("/tmp/config.toml"), 0o100644);
        assert_eq!((check.verdict, check.fix.as_deref()), (Verdict::Warn, Some("chmod 600 /tmp/config.toml")));
    }

    #[test]
    fn tls_on_says_require_checks_the_certificate_like_verify_full() {
        let check = tls(&connection(""));
        assert_eq!(check.verdict, Verdict::Ok);
        assert!(check.found.starts_with("on, as `sslmode=require` asks") && check.found.contains("`verify-full`"), "{}", check.found);
    }

    #[test]
    fn a_private_ca_fails_the_tls_check() {
        let check = tls(&local("postgres://nina@localhost/shop?sslmode=verify-full&sslrootcert=ca.pem", ""));
        assert_eq!(check.verdict, Verdict::Fail);
    }

    #[test]
    fn tls_off_to_this_machine_is_fine() {
        assert_eq!(tls(&local("postgres://nina@localhost/shop", "")), Check::ok("tls", "off: the host is this machine"));
    }

    #[test]
    fn tls_off_on_purpose_to_a_remote_host_is_a_warning() {
        let remote = local("postgres://nina@db.example.com/shop", "tls = \"off\"");
        assert_eq!(tls(&remote).verdict, Verdict::Warn);
    }

    #[test]
    fn tls_off_through_a_tunnel_to_this_machine_is_a_warning() {
        let tunnelled = local("postgres://nina@localhost/shop", "tls = \"off\"\ntunnel = { command = [\"proxy\", \"{port}\"] }");
        assert_eq!(tls(&tunnelled).verdict, Verdict::Warn);
    }

    fn tunnel(text: &str) -> TunnelSpec {
        connection(text).tunnel().unwrap().clone()
    }

    #[test]
    fn a_started_tunnel_names_its_programs_and_never_their_arguments() {
        let spec = tunnel(
            "tunnel = { command = [\"ssh\", \"-L\", \"{port}:localhost:5432\", \"db.example.com\"], before = [\"wake\", \"bastion\"] }",
        );
        assert_eq!(tunnel_started(&spec, LocalPort::new(49213)).found, "`before` (`wake`) ran, then `ssh` answers on :49213");
    }

    #[test]
    fn a_port_still_answering_after_the_stop_is_a_warning_about_controlmaster() {
        let check = tunnel_stopped(LocalPort::new(49213), false);
        assert_eq!(check.verdict, Verdict::Warn);
        assert!(check.found.contains("ControlMaster"), "{}", check.found);
    }

    fn facts(in_effect_ms: u64, server_ms: u64) -> ServerFacts {
        ServerFacts {
            user: "nina".into(),
            version: "17.2".into(),
            statement_timeout: StatementTimeout::from_millis(in_effect_ms),
            server_statement_timeout: StatementTimeout::from_millis(server_ms),
        }
    }

    #[test]
    fn the_tier_timeout_in_effect_is_fine() {
        assert_eq!(timeout(&connection(""), &facts(30_000, 0)), Check::ok("timeout", "30s, the production tier's"));
    }

    #[test]
    fn a_stricter_server_timeout_in_effect_says_it_won_over_the_tier() {
        assert_eq!(timeout(&connection(""), &facts(5_000, 5_000)).found, "5s, the server's, stricter than the production tier's 30s");
    }

    #[test]
    fn the_connections_own_timeout_in_effect_is_fine() {
        assert_eq!(timeout(&connection("statement_timeout = \"2min\""), &facts(120_000, 5_000)).found, "2min, the connection's own");
    }

    #[test]
    fn a_timeout_other_than_the_one_pgbg_set_is_a_warning() {
        let check = timeout(&connection(""), &facts(0, 0));
        assert_eq!((check.verdict, check.found.as_str()), (Verdict::Warn, "0s in effect, not the 30s pgbg set"));
    }

    #[test]
    fn a_role_that_writes_nothing_is_read_only() {
        assert_eq!(role(WritePolicy::ReadOnly, &RoleRights::default()), Check::ok("role", "read only: it can write no table"));
    }

    fn writer() -> RoleRights {
        let tables = Named { first: vec!["public.customers".into(), "public.orders".into()], count: 14 };
        RoleRights {
            roles: vec![RolePowers { role: "nina".into(), flags: vec![RoleFlag::CreateDatabase], tables, ..RolePowers::default() }],
        }
    }

    #[test]
    fn a_role_that_writes_on_a_read_only_connection_is_a_warning_naming_what_it_writes() {
        let check = role(WritePolicy::ReadOnly, &writer());
        assert_eq!(check.found, "not read only: `nina` creates databases, writes 14 tables: public.customers, public.orders and 12 more");
        assert_eq!(check.fix.as_deref(), Some(READ_ONLY_ROLE));
    }

    #[test]
    fn a_role_that_writes_on_a_connection_that_allows_writes_is_fine() {
        assert_eq!(role(WritePolicy::Allowed, &writer()).verdict, Verdict::Ok);
    }

    #[test]
    fn each_role_that_writes_is_named_with_what_it_writes() {
        let sequences = Named { first: vec!["public.orders_id_seq".into()], count: 1 };
        let schemas = Named { first: vec!["public".into()], count: 1 };
        let rights = RoleRights {
            roles: vec![
                RolePowers { role: "nina".into(), sequences, ..RolePowers::default() },
                RolePowers { role: "PUBLIC".into(), schemas, ..RolePowers::default() },
            ],
        };
        let found = "not read only: `nina` advances 1 sequence: public.orders_id_seq; `PUBLIC` creates objects in 1 schema: public";
        assert_eq!(role(WritePolicy::ReadOnly, &rights).found, found);
    }

    #[test]
    fn a_superuser_says_so_first() {
        let powers = RolePowers { role: "nina".into(), flags: vec![RoleFlag::Superuser, RoleFlag::CreateRole], ..RolePowers::default() };
        let rights = RoleRights { roles: vec![powers] };
        assert_eq!(role(WritePolicy::ReadOnly, &rights).found, "not read only: `nina` is a superuser, creates roles");
    }

    #[test]
    fn a_role_a_read_only_transaction_holds_whole_is_fine() {
        assert_eq!(beyond_read_only(WritePolicy::ReadOnly, &ReadOnlyGaps::default()).verdict, Verdict::Ok);
    }

    fn gaps() -> ReadOnlyGaps {
        ReadOnlyGaps {
            signals_sessions: true,
            dblink_functions: vec!["dblink_exec".into()],
            granted_settings: vec!["session_replication_role".into()],
        }
    }

    #[test]
    fn a_role_that_reaches_past_a_read_only_transaction_is_a_warning_naming_each_way() {
        let check = beyond_read_only(WritePolicy::ReadOnly, &gaps());
        let found = "it can cancel or end other sessions (`pg_signal_backend`), call `dblink_exec`, which opens a session of its own \
                     that may write, and set `session_replication_role`";
        assert_eq!((check.verdict, check.found.as_str()), (Verdict::Warn, found));
    }

    #[test]
    fn a_role_a_read_only_transaction_holds_can_still_end_sessions_of_its_own_role() {
        let found = beyond_read_only(WritePolicy::ReadOnly, &ReadOnlyGaps::default()).found;
        assert!(found.contains("cancel or end sessions of its own role"), "{found}");
    }

    #[test]
    fn a_role_without_pg_signal_backend_can_still_end_sessions_of_its_own_role() {
        let gaps = ReadOnlyGaps { signals_sessions: false, ..gaps() };
        let found = beyond_read_only(WritePolicy::ReadOnly, &gaps).found;
        assert!(found.ends_with("; it can still cancel or end sessions of its own role, as any role can"), "{found}");
    }

    #[test]
    fn a_role_that_reaches_past_a_read_only_transaction_is_fine_where_writes_are_allowed() {
        assert_eq!(beyond_read_only(WritePolicy::Allowed, &gaps()).verdict, Verdict::Ok);
    }
}
