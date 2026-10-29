//! The keys of a result's columns: which are the primary key, and where a foreign key points, read from the catalog,
//! and the read of the row a foreign key points to.
//! Pure, and only sure when the statement reads one table: a result column is that table's column of the same name.
use crate::complete;
use crate::db::model::{Catalog, Column, ForeignKey, ParamValue, Params, SchemaNode, Statement, Table, Value, qualified, sql_name};
use crate::query::params::ParamName;
use std::collections::BTreeMap;

/// The row a foreign key points to: its table, and the column there that each column of the key equals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Target {
    pub(crate) schema: String,
    pub(crate) table: String,
    /// One for each column of the key, most keys having one.
    pub(crate) pairs: Vec<KeyPair>,
}

/// A column of a foreign key: its place in the result, and the column of the target it equals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KeyPair {
    pub(crate) at: usize,
    pub(crate) column: String,
}

/// What a result's column is in the table its statement reads.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ColumnKey {
    pub(crate) is_primary: bool,
    /// Where it points, when it is a column of a foreign key whose every column the result holds once.
    pub(crate) points_to: Option<Target>,
}

/// The most tables a row's detail lists as pointing to it, each counted with one read.
const REFERRERS_SHOWN: usize = 12;
/// How far a count of the rows pointing to a row goes before it says only that there are more.
pub(crate) const COUNTED_ROWS: u64 = 1000;

/// A table of the catalog, with the schema that holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TableIn<'a> {
    pub(crate) schema: &'a str,
    pub(crate) table: &'a Table,
}

/// A foreign key of another table that points to a row: that table, and each column of the key with the row's value
/// it equals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Referrer {
    pub(crate) schema: String,
    pub(crate) table: String,
    pub(crate) pairs: Vec<Pointing>,
}

/// A column of a key that points to a row, and the value of that row it equals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Pointing {
    pub(crate) column: String,
    pub(crate) value: ParamValue,
}

/// A table of a trail of rows, and the columns its row equals in the row before it; none for the first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hop {
    pub(crate) schema: String,
    pub(crate) table: String,
    pub(crate) on: Vec<Joined>,
}

/// A column of the table before a hop, and the column of the hop's table it equals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Joined {
    pub(crate) from: String,
    pub(crate) to: String,
}

/// A statement with `{{name}}` parameters, and the value each of them takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Prefilled {
    pub(crate) text: String,
    pub(crate) values: BTreeMap<ParamName, ParamValue>,
}

/// A read-only statement and the values bound to its `$n`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Lookup {
    pub(crate) statement: Statement,
    pub(crate) params: Params,
}

/// The key of each of `columns`, the result of `statement` read with `catalog`; none when the statement reads more than
/// one table, or one the catalog does not hold.
pub(crate) fn column_keys(catalog: &Catalog, statement: &Statement, columns: &[Column]) -> Vec<ColumnKey> {
    let Some(TableIn { table, .. }) = one_table_read(catalog, statement) else { return vec![ColumnKey::default(); columns.len()] };
    columns.iter().map(|column| key_of(table, column.name.as_str(), columns)).collect()
}

fn key_of(table: &Table, name: &str, columns: &[Column]) -> ColumnKey {
    let is_primary = table.indexes.iter().any(|index| index.is_primary && index.columns.iter().any(|key| key == name));
    let keys = table.foreign_keys.iter().filter(|key| key.columns.iter().any(|column| column == name));
    ColumnKey { is_primary, points_to: keys.into_iter().find_map(|key| target_of(key, columns)) }
}

/// Where `key` points from `columns`, when they hold each of its columns once.
fn target_of(key: &ForeignKey, columns: &[Column]) -> Option<Target> {
    let pair = |(from, to): (&String, &String)| Some(KeyPair { at: only_column(columns, from)?, column: to.clone() });
    let pairs: Option<Vec<KeyPair>> = key.columns.iter().zip(&key.target_columns).map(pair).collect();
    Some(Target { schema: key.target_schema.clone(), table: key.target_table.clone(), pairs: pairs? })
}

/// The place of the one column named `name`; none when `columns` hold no such column, or several.
fn only_column(columns: &[Column], name: &str) -> Option<usize> {
    let mut named = columns.iter().enumerate().filter(|(_, column)| column.name.as_str() == name).map(|(at, _)| at);
    let first = named.next()?;
    named.next().is_none().then_some(first)
}

impl Target {
    /// The hop the key makes from a row of `columns` to the row it points to.
    pub(crate) fn hop(&self, columns: &[Column]) -> Hop {
        let joined = |pair: &KeyPair| Some(Joined { from: columns.get(pair.at)?.name.as_str().to_owned(), to: pair.column.clone() });
        Hop { schema: self.schema.clone(), table: self.table.clone(), on: self.pairs.iter().filter_map(joined).collect() }
    }

    /// The place in the result of the key's first column.
    pub(crate) fn first_column(&self) -> usize {
        self.pairs.iter().map(|pair| pair.at).min().unwrap_or_default()
    }

    /// The read of the row the key of `row` points to, its values bound, never put in its text; none when one of them
    /// is NULL, which points nowhere.
    pub(crate) fn lookup(&self, row: &[Value]) -> Option<Lookup> {
        let values: Option<Vec<_>> = self.pairs.iter().map(|pair| row.get(pair.at)?.as_param()).collect();
        let equal = self.pairs.iter().enumerate().map(|(at, pair)| format!("{} = ${}", sql_name(&pair.column), at + 1));
        let condition = equal.collect::<Vec<_>>().join(" AND ");
        let statement = Statement::new(format!("SELECT * FROM {} WHERE {condition}", qualified(&self.schema, &self.table)));
        Some(Lookup { statement, params: Params::new(values?.into_iter().map(Some).collect(), vec![]) })
    }
}

/// The table `statement` reads, when it names exactly one, found as PostgreSQL resolves its name: with its schema, or
/// bare in the first schema of the search path that holds it.
pub(crate) fn one_table_read<'a>(catalog: &'a Catalog, statement: &Statement) -> Option<TableIn<'a>> {
    let [name] = complete::tables_named(statement.as_str()).try_into().ok()?;
    match name.split_once('.') {
        Some((schema, table)) => table_in(catalog, schema, table),
        None => catalog.search_path.iter().find_map(|schema| table_in(catalog, schema, &name)),
    }
}

/// The table `name` of the schema `schema`, as the catalog spells them.
pub(crate) fn table_in<'a>(catalog: &'a Catalog, schema: &str, name: &str) -> Option<TableIn<'a>> {
    let node = catalog.schema.schemas.iter().find(|node| node.name == schema)?;
    node.tables.iter().find(|table| table.name == name).map(|table| TableIn { schema: &node.name, table })
}

/// The keys of other tables that point to a row of the table `pointed`, whose `columns` hold `row`: each key whose
/// target columns the row holds, its values not NULL, the first `REFERRERS_SHOWN` in the catalog's order.
pub(crate) fn referrers(catalog: &Catalog, pointed: TableIn<'_>, columns: &[Column], row: &[Value]) -> Vec<Referrer> {
    let is_pointed = |key: &&ForeignKey| key.target_schema == pointed.schema && key.target_table == pointed.table.name;
    let tables = catalog.schema.schemas.iter().flat_map(|node| node.tables.iter().map(move |table| (node, table)));
    let keys = tables.flat_map(|(node, table)| table.foreign_keys.iter().filter(is_pointed).map(move |key| (node, table, key)));
    let referrer = |(node, table, key): (&SchemaNode, &Table, &ForeignKey)| {
        let value = |target: &String| row.get(only_column(columns, target)?)?.as_param();
        let values: Option<Vec<ParamValue>> = key.target_columns.iter().map(value).collect();
        let pairs = key.columns.iter().cloned().zip(values?).map(|(column, value)| Pointing { column, value }).collect();
        Some(Referrer { schema: node.name.clone(), table: table.name.clone(), pairs })
    };
    keys.filter_map(referrer).take(REFERRERS_SHOWN).collect()
}

impl Referrer {
    /// How many of its rows point to the row, counted up to `COUNTED_ROWS` and one more, so a big table costs no more
    /// than a small one.
    pub(crate) fn count(&self) -> Lookup {
        let statement = format!(
            "SELECT count(*) FROM (SELECT FROM {} WHERE {} LIMIT {}) AS pointing",
            self.sql_name(),
            self.condition(),
            COUNTED_ROWS + 1
        );
        let values = self.pairs.iter().map(|pair| Some(pair.value.clone())).collect();
        Lookup { statement: Statement::new(statement), params: Params::new(values, vec![]) }
    }

    /// Its rows that point to the row, as a statement whose `{{name}}` parameters hold the row's values.
    pub(crate) fn rows(&self) -> Option<Prefilled> {
        let (condition, values) = param_condition("", &self.pairs)?;
        Some(Prefilled { text: format!("SELECT * FROM {} WHERE {condition}", self.sql_name()), values })
    }

    fn sql_name(&self) -> String {
        qualified(&self.schema, &self.table)
    }

    fn condition(&self) -> String {
        let equal = self.pairs.iter().enumerate().map(|(at, pair)| format!("{} = ${}", sql_name(&pair.column), at + 1));
        equal.collect::<Vec<_>>().join(" AND ")
    }
}

/// The statement joining the tables of a trail, the first one's row picked by `key` as `{{name}}` parameters: each
/// table called by the alias completion gives it, joined to the one before on the key that led to it.
pub(crate) fn trail_join(tables: &[Hop], key: &[Pointing]) -> Option<Prefilled> {
    let aliases = tables.iter().fold(Vec::<String>::new(), |mut aliases, hop| {
        let taken: Vec<&str> = aliases.iter().map(String::as_str).collect();
        let alias = complete::alias_of(&hop.table, &taken);
        aliases.push(alias);
        aliases
    });
    let [first, rest @ ..] = tables else { return None };
    let joins = rest.iter().zip(aliases.windows(2)).map(|(hop, pair)| {
        let equal = hop.on.iter().map(|on| format!("{}.{} = {}.{}", pair[1], sql_name(&on.to), pair[0], sql_name(&on.from)));
        format!("\nJOIN {} {} ON {}", qualified(&hop.schema, &hop.table), pair[1], equal.collect::<Vec<_>>().join(" AND "))
    });
    let joins: String = joins.collect();
    let (condition, values) = param_condition(&format!("{}.", aliases[0]), key)?;
    let text = format!("SELECT *\nFROM {} {}{joins}\nWHERE {condition}", qualified(&first.schema, &first.table), aliases[0]);
    Some(Prefilled { text, values })
}

/// `column = {{name}}` for each of `pairs`, each column after `qualifier`, joined with `AND`, and the value each name
/// takes; a column that is no parameter name gives `key_<n>`.
fn param_condition(qualifier: &str, pairs: &[Pointing]) -> Option<(String, BTreeMap<ParamName, ParamValue>)> {
    let name = |(at, pair): (usize, &Pointing)| pair.column.parse().or_else(|_| format!("key_{}", at + 1).parse()).ok();
    let names: Vec<ParamName> = pairs.iter().enumerate().map(name).collect::<Option<_>>()?;
    let equal = pairs.iter().zip(&names).map(|(pair, name)| format!("{qualifier}{} = {{{{{name}}}}}", sql_name(&pair.column)));
    let condition = equal.collect::<Vec<_>>().join(" AND ");
    Some((condition, names.into_iter().zip(pairs.iter().map(|pair| pair.value.clone())).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::WritePolicy;
    use crate::db::guard;
    use crate::db::model::ColumnKind;
    use crate::fixtures::{self, column, shop_catalog};
    use crate::query::params::{self, Given};

    fn keys(statement: &str, names: &[&str]) -> Vec<ColumnKey> {
        let columns: Vec<Column> = names.iter().map(|name| column(name, ColumnKind::Int)).collect();
        column_keys(&shop_catalog(), &Statement::new(statement), &columns)
    }

    /// A key of the one column at `at` pointing to `id` of `schema.table`.
    fn to_id(schema: &str, table: &str, at: usize) -> Target {
        Target { schema: schema.to_owned(), table: table.to_owned(), pairs: vec![KeyPair { at, column: "id".to_owned() }] }
    }

    #[test]
    fn a_column_of_the_primary_key_of_the_one_table_read_is_primary() {
        assert!(keys("SELECT * FROM orders", &["id"])[0].is_primary);
    }

    #[test]
    fn a_column_with_a_foreign_key_points_to_its_target() {
        let keys = keys("SELECT id, customer_id FROM orders", &["id", "customer_id"]);
        assert_eq!(keys[1].points_to, Some(to_id("public", "customers", 1)));
    }

    #[test]
    fn a_table_named_with_its_schema_is_found_in_that_schema() {
        assert_eq!(keys("SELECT * FROM billing.invoices", &["order_id"])[0].points_to, Some(to_id("public", "orders", 0)));
    }

    #[test]
    fn a_key_column_the_result_holds_twice_points_nowhere() {
        assert_eq!(keys("SELECT customer_id, customer_id FROM orders", &["customer_id", "customer_id"])[0].points_to, None);
    }

    #[test]
    fn each_column_of_a_key_of_several_points_to_the_whole_key() {
        let mut catalog = shop_catalog();
        let key = &mut catalog.schema.schemas[1].tables[0].foreign_keys[0];
        key.columns.push("total".to_owned());
        key.target_columns.push("region".to_owned());
        let columns = ["id", "customer_id", "total"].map(|name| column(name, ColumnKind::Int));
        let keys = column_keys(&catalog, &Statement::new("SELECT * FROM orders"), &columns);
        let region = KeyPair { at: 2, column: "region".to_owned() };
        let both = Target { pairs: vec![KeyPair { at: 1, column: "id".to_owned() }, region], ..to_id("public", "customers", 1) };
        assert_eq!([&keys[1].points_to, &keys[2].points_to], [&Some(both.clone()), &Some(both)]);
    }

    #[test]
    fn the_lookup_binds_the_rows_key_and_puts_no_value_in_its_text() {
        let row = [Value::Int(7), Value::Text("3'; DROP TABLE orders".into())];
        let lookup = to_id("public", "customers", 1).lookup(&row).unwrap();
        assert_eq!(lookup.statement, Statement::new("SELECT * FROM public.customers WHERE id = $1"));
        assert_eq!(lookup.params, Params::new(vec![Some(ParamValue::new("3'; DROP TABLE orders"))], vec![]));
    }

    #[test]
    fn a_null_key_looks_nothing_up() {
        assert_eq!(to_id("public", "customers", 1).lookup(&[Value::Int(12), Value::Null]), None);
    }

    #[test]
    fn a_bare_table_outside_the_search_path_is_not_found() {
        assert_eq!(keys("SELECT * FROM invoices", &["order_id"]), [ColumnKey::default()]);
    }

    #[test]
    fn a_statement_reading_two_tables_has_no_keys() {
        assert_eq!(keys("SELECT * FROM orders JOIN customers ON true", &["id"]), [ColumnKey::default()]);
    }

    #[test]
    fn a_column_the_table_does_not_hold_has_no_key() {
        assert_eq!(keys("SELECT count(*) AS total FROM orders", &["total"]), [ColumnKey::default()]);
    }

    fn orders_referrers(row: &[Value]) -> Vec<Referrer> {
        let catalog = shop_catalog();
        let columns = ["id", "customer_id"].map(|name| column(name, ColumnKind::Int));
        referrers(&catalog, table_in(&catalog, "public", "orders").unwrap(), &columns, row)
    }

    fn invoices_of_order_7() -> Referrer {
        let pairs = vec![Pointing { column: "order_id".to_owned(), value: ParamValue::new("7") }];
        Referrer { schema: "billing".to_owned(), table: "invoices".to_owned(), pairs }
    }

    #[test]
    fn a_table_whose_key_points_to_the_row_refers_to_it_by_the_rows_values() {
        assert_eq!(orders_referrers(&[Value::Int(7), Value::Int(3)]), [invoices_of_order_7()]);
    }

    #[test]
    fn a_row_whose_pointed_value_is_null_has_no_referrer() {
        assert_eq!(orders_referrers(&[Value::Null, Value::Int(3)]), []);
    }

    #[test]
    fn a_count_stops_one_row_past_the_cap_and_binds_the_value() {
        let count = invoices_of_order_7().count();
        let statement = "SELECT count(*) FROM (SELECT FROM billing.invoices WHERE order_id = $1 LIMIT 1001) AS pointing";
        assert_eq!((count.statement, count.params), (Statement::new(statement), Params::new(vec![Some(ParamValue::new("7"))], vec![])));
    }

    #[test]
    fn the_rows_pointing_to_a_row_take_its_values_as_parameters() {
        let rows = invoices_of_order_7().rows().unwrap();
        assert_eq!(rows.text, "SELECT * FROM billing.invoices WHERE order_id = {{order_id}}");
        assert_eq!(rows.values, [("order_id".parse().unwrap(), ParamValue::new("7"))].into());
    }

    #[test]
    fn a_column_that_is_no_parameter_name_takes_one_by_its_place() {
        let referrer =
            Referrer { pairs: vec![Pointing { column: "Order Id".to_owned(), value: ParamValue::new("7") }], ..invoices_of_order_7() };
        assert_eq!(referrer.rows().unwrap().text, "SELECT * FROM billing.invoices WHERE \"Order Id\" = {{key_1}}");
    }

    fn hop(schema: &str, table: &str, from: &str) -> Hop {
        let on = if from.is_empty() { vec![] } else { vec![Joined { from: from.to_owned(), to: "id".to_owned() }] };
        Hop { schema: schema.to_owned(), table: table.to_owned(), on }
    }

    fn order_7() -> Vec<Pointing> {
        vec![Pointing { column: "id".to_owned(), value: ParamValue::new("7") }]
    }

    #[test]
    fn a_trail_joins_each_table_to_the_one_before_on_the_key_that_led_to_it() {
        let trail = [hop("public", "orders", ""), hop("public", "customers", "customer_id"), hop("public", "users", "user_id")];
        let join = trail_join(&trail, &order_7()).unwrap();
        let text = "SELECT *\nFROM public.orders o\nJOIN public.customers c ON c.id = o.customer_id\nJOIN public.users u ON u.id = c.user_id\nWHERE o.id = {{id}}";
        assert_eq!((join.text.as_str(), join.values), (text, [("id".parse().unwrap(), ParamValue::new("7"))].into()));
    }

    #[test]
    fn a_table_of_the_trail_whose_initials_are_taken_gets_a_number() {
        let trail = [hop("public", "orders", ""), hop("public", "orders", "parent_id")];
        let join = trail_join(&trail, &order_7()).unwrap();
        assert!(join.text.contains("JOIN public.orders o2 ON o2.id = o.parent_id"), "{}", join.text);
    }

    /// A fresh `table` holding three rows that point to 7 and one that points to 8.
    async fn pointing_rows(table: &str) -> Referrer {
        fixtures::run(WritePolicy::Allowed, &format!("DROP TABLE IF EXISTS {table}")).await.unwrap();
        fixtures::run(WritePolicy::Allowed, &format!("CREATE TABLE {table} (order_id int)")).await.unwrap();
        fixtures::run(WritePolicy::Allowed, &format!("INSERT INTO {table} VALUES (7), (7), (7), (8)")).await.unwrap();
        Referrer { schema: "public".to_owned(), table: table.to_owned(), ..invoices_of_order_7() }
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn the_count_reads_how_many_rows_point_to_the_row() {
        let count = pointing_rows("pgbg_referrers_counted").await.count();
        let admitted = guard::admit(count.statement, count.params, WritePolicy::ReadOnly).unwrap();
        let read = fixtures::session(WritePolicy::ReadOnly).await.run(&admitted).await.unwrap();
        assert_eq!(read.rows(), [[Value::Int(3)]]);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn the_rows_pointing_to_the_row_run_with_their_parameters_bound() {
        let rows = pointing_rows("pgbg_referrers_run").await.rows().unwrap();
        let admitted = params::admit(&Statement::new(rows.text), &Given::typed(rows.values), &[], WritePolicy::ReadOnly).unwrap();
        let read = fixtures::session(WritePolicy::ReadOnly).await.run(&admitted).await.unwrap();
        assert_eq!(read.rows().len(), 3);
    }

    #[tokio::test]
    #[ignore = "needs PGBG_TEST_DATABASE_URL"]
    async fn the_join_of_a_trail_reads_the_row_with_the_rows_it_led_to() {
        let made =
            ["pgbg_trail_users (id int)", "pgbg_trail_customers (id int, user_id int)", "pgbg_trail_orders (id int, customer_id int)"];
        for table in made {
            let name = table.split_once(' ').map_or(table, |(name, _)| name);
            fixtures::run(WritePolicy::Allowed, &format!("DROP TABLE IF EXISTS {name}")).await.unwrap();
            fixtures::run(WritePolicy::Allowed, &format!("CREATE TABLE {table}")).await.unwrap();
        }
        let rows = "INSERT INTO pgbg_trail_users VALUES (5); INSERT INTO pgbg_trail_customers VALUES (3, 5); INSERT INTO pgbg_trail_orders VALUES (7, 3), (8, 3)";
        for insert in rows.split("; ") {
            fixtures::run(WritePolicy::Allowed, insert).await.unwrap();
        }
        let trail = [
            hop("public", "pgbg_trail_orders", ""),
            hop("public", "pgbg_trail_customers", "customer_id"),
            hop("public", "pgbg_trail_users", "user_id"),
        ];
        let join = trail_join(&trail, &order_7()).unwrap();
        let admitted = params::admit(&Statement::new(join.text), &Given::typed(join.values), &[], WritePolicy::ReadOnly).unwrap();
        let read = fixtures::session(WritePolicy::ReadOnly).await.run(&admitted).await.unwrap();
        assert_eq!(read.rows(), [[Value::Int(7), Value::Int(3), Value::Int(3), Value::Int(5), Value::Int(5)]]);
    }
}
