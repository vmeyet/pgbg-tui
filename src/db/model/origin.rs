//! Where a result's column was read from, as the server says: a column of a table, by numbers the catalog gives both.

/// A table's oid: the catalog's number for it, whatever its schema or name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TableOid(u32);

impl TableOid {
    pub(crate) const fn new(oid: u32) -> Self {
        Self(oid)
    }

    pub(crate) const fn get(self) -> u32 {
        self.0
    }
}

/// A column's number in its table, `attnum` in the catalog: 1 for the first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ColumnNumber(i16);

impl ColumnNumber {
    pub(crate) const fn new(number: i16) -> Self {
        Self(number)
    }
}

/// The column of a table a result's column holds as is; a computed column has none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Origin {
    pub(crate) table: TableOid,
    pub(crate) column: ColumnNumber,
}

impl Origin {
    /// The origin the server gave a result's column, when it gave both its table and its column.
    pub(crate) fn of(table: Option<u32>, column: Option<i16>) -> Option<Self> {
        Some(Self { table: TableOid::new(table?), column: ColumnNumber::new(column?) })
    }
}
