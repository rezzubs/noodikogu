//! A generic tabular widget.

pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<TableRow>,
    pub selected: Option<usize>,
}

impl Table {
    fn view(&self) {
        todo!()
    }
}

pub struct TableRow {
    pub columns: Vec<String>,
    pub content: Option<TableContent>,
}

pub enum TableContent {
    Text(String),
    Table(Box<Table>),
}
