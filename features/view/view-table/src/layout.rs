//! Pure layout pre-pass: filter → sort → optionally group.
//!
//! Output is a flat `Vec<DisplayRow>` the renderer walks top-to-
//! bottom. Group headers and data rows live in the same list so
//! sticky positioning and keyboard nav can stay simple.

use indexmap::IndexMap;

use crate::store::TableState;
use crate::types::{CellValue, ColumnId, Row, SortDir};

/// One entry in the rendered list — either a group header or a row.
#[derive(Clone, Debug, PartialEq)]
pub enum DisplayRow {
    /// `key` is the lowercase sort-key form of the group cell;
    /// `label` is the original cell text (capitalization preserved).
    /// `count` is the number of rows in the group, regardless of
    /// `collapsed`.
    GroupHeader {
        key: String,
        label: String,
        count: usize,
        collapsed: bool,
    },
    Row(Row),
}

/// Filter → sort → group. Returns the flat display list.
pub fn layout(state: &TableState) -> Vec<DisplayRow> {
    // Filter.
    let filtered: Vec<&Row> = state
        .rows
        .iter()
        .filter(|r| matches_filters(r, &state.filters))
        .collect();

    // Sort.
    let mut sorted: Vec<Row> = filtered.into_iter().cloned().collect();
    if let Some((col, dir)) = state.sort {
        sorted.sort_by(|a, b| {
            let ak = a
                .cells
                .get(&col)
                .map(CellValue::as_sort_key)
                .unwrap_or_default();
            let bk = b
                .cells
                .get(&col)
                .map(CellValue::as_sort_key)
                .unwrap_or_default();
            match dir {
                SortDir::Asc => ak.cmp(&bk),
                SortDir::Desc => bk.cmp(&ak),
            }
        });
    }

    // Group.
    let Some(group_col) = state.group_by else {
        return sorted.into_iter().map(DisplayRow::Row).collect();
    };

    // Bucket by lowercase group key while preserving first-seen
    // order (matches the visual order the user expects after sort).
    let mut buckets: IndexMap<String, (String, Vec<Row>)> = IndexMap::new();
    for r in sorted {
        let cell = r.cells.get(&group_col).cloned().unwrap_or(CellValue::Empty);
        let key = cell.as_sort_key();
        let label = match &cell {
            CellValue::Empty => "—".to_string(),
            CellValue::Text(s) | CellValue::Select(s) => s.clone(),
            CellValue::Number(n) => format!("{n}"),
            CellValue::Date(d) => d.format("%Y-%m-%d").to_string(),
            CellValue::Checkbox(b) => if *b { "✓" } else { "—" }.to_string(),
        };
        buckets
            .entry(key)
            .or_insert_with(|| (label, Vec::new()))
            .1
            .push(r);
    }

    let mut out: Vec<DisplayRow> = Vec::with_capacity(buckets.len() * 4);
    for (key, (label, rows)) in buckets {
        let collapsed = state.collapsed_groups.iter().any(|k| k == &key);
        out.push(DisplayRow::GroupHeader {
            key: key.clone(),
            label,
            count: rows.len(),
            collapsed,
        });
        if !collapsed {
            out.extend(rows.into_iter().map(DisplayRow::Row));
        }
    }
    out
}

/// `true` if every active filter matches its column's cell text.
fn matches_filters(row: &Row, filters: &IndexMap<ColumnId, String>) -> bool {
    filters.iter().all(|(col, q)| {
        let hay = row
            .cells
            .get(col)
            .map(CellValue::as_filter_str)
            .unwrap_or_default();
        hay.contains(q)
    })
}

#[cfg(test)]
#[allow(clippy::match_wildcard_for_single_variants)]
mod tests {
    use super::*;
    use crate::types::{Column, ColumnType};

    fn col(label: &str, ty: ColumnType) -> Column {
        Column::new(label, ty)
    }

    fn row(col_id: ColumnId, val: CellValue) -> Row {
        Row::new().with(col_id, val)
    }

    #[test]
    fn filter_text_substring() {
        let c = col("name", ColumnType::Text);
        let state = TableState {
            columns: vec![c.clone()],
            rows: vec![
                row(c.id, CellValue::Text("Alice".into())),
                row(c.id, CellValue::Text("Bob".into())),
                row(c.id, CellValue::Text("Carol".into())),
            ],
            filters: [(c.id, "o".into())].into_iter().collect(),
            ..Default::default()
        };
        let out = layout(&state);
        assert_eq!(out.len(), 2); // Bob, Carol
    }

    #[test]
    fn sort_asc_then_desc() {
        let c = col("n", ColumnType::Number);
        let mut state = TableState {
            columns: vec![c.clone()],
            rows: vec![
                row(c.id, CellValue::Number(3.0)),
                row(c.id, CellValue::Number(1.0)),
                row(c.id, CellValue::Number(2.0)),
            ],
            sort: Some((c.id, SortDir::Asc)),
            ..Default::default()
        };
        let asc = layout(&state);
        let asc_vals: Vec<f64> = asc
            .iter()
            .filter_map(|r| match r {
                DisplayRow::Row(r) => r.cells.get(&c.id).map(|v| match v {
                    CellValue::Number(n) => *n,
                    _ => f64::NAN,
                }),
                _ => None,
            })
            .collect();
        assert_eq!(asc_vals, vec![1.0, 2.0, 3.0]);

        state.sort = Some((c.id, SortDir::Desc));
        let desc = layout(&state);
        let desc_vals: Vec<f64> = desc
            .iter()
            .filter_map(|r| match r {
                DisplayRow::Row(r) => r.cells.get(&c.id).map(|v| match v {
                    CellValue::Number(n) => *n,
                    _ => f64::NAN,
                }),
                _ => None,
            })
            .collect();
        assert_eq!(desc_vals, vec![3.0, 2.0, 1.0]);
    }

    #[test]
    fn group_by_select_builds_headers_with_counts() {
        let c = col("status", ColumnType::Select);
        let state = TableState {
            columns: vec![c.clone()],
            rows: vec![
                row(c.id, CellValue::Select("todo".into())),
                row(c.id, CellValue::Select("done".into())),
                row(c.id, CellValue::Select("todo".into())),
            ],
            group_by: Some(c.id),
            ..Default::default()
        };
        let out = layout(&state);
        let headers: Vec<&DisplayRow> = out
            .iter()
            .filter(|r| matches!(r, DisplayRow::GroupHeader { .. }))
            .collect();
        assert_eq!(headers.len(), 2);
        let counts: Vec<usize> = headers
            .iter()
            .map(|h| match h {
                DisplayRow::GroupHeader { count, .. } => *count,
                _ => 0,
            })
            .collect();
        // todo first (3 < d so "todo" > "done" alphabetically but
        // first-seen order = todo, done)
        assert_eq!(counts, vec![2, 1]);
    }
}
