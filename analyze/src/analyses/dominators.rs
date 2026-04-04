use std::io;

use twiggy_ir as ir;

use crate::{
    analyses::garbage,
    formats::table::{Align, Table},
    OutputFormat,
};

#[derive(Clone, Debug)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct Options {
    /// The name of the function whose dominator subtree should be printed.
    #[cfg_attr(feature = "clap", arg(id = "items"))]
    pub item_names: Vec<String>,

    /// Whether or not `items` should be treated as regular expressions.
    #[cfg_attr(feature = "clap", arg(long = "regex"))]
    pub using_regexps: bool,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct EmitOptions {
    /// The format the output should be written in.
    #[cfg_attr(feature = "clap", arg(short, long, default_value_t))]
    pub format: OutputFormat,

    /// The maximum depth to print the dominators tree.
    #[cfg_attr(feature = "clap", arg(short = 'd'))]
    pub max_depth: Option<u32>,

    /// The maximum number of rows, regardless of depth in the tree, to display.
    #[cfg_attr(feature = "clap", arg(short = 'r'))]
    pub max_rows: Option<u32>,
}

pub struct Report<'a> {
    item_ids: Vec<ir::Id>,
    items: &'a ir::Items,
    unreachable_items_summary: Option<UnreachableItemsSummary>,
}

struct UnreachableItemsSummary {
    count: usize,
    size: u32,
    size_percent: f64,
}

impl Report<'_> {
    pub fn emit(&self, opts: EmitOptions, dest: impl io::Write) -> anyhow::Result<()> {
        match opts.format {
            OutputFormat::Text => self.emit_text(opts.max_depth, opts.max_rows, dest),
            #[cfg(feature = "emit_json")]
            OutputFormat::Json => self.emit_json(dest),
            #[cfg(feature = "emit_csv")]
            OutputFormat::Csv => self.emit_csv(dest),
        }
    }

    fn emit_text(
        &self,
        max_depth: Option<u32>,
        max_rows: Option<u32>,
        mut dest: impl io::Write,
    ) -> anyhow::Result<()> {
        let mut table = Table::with_header(vec![
            (Align::Right, "Retained Bytes".to_string()),
            (Align::Right, "Retained %".to_string()),
            (Align::Left, "Dominator Tree".to_string()),
        ]);

        let mut row = 0 as u32;

        fn recursive_add_rows(
            table: &mut Table,
            items: &ir::Items,
            depth: u32,
            mut row: &mut u32,
            id: ir::Id,
            max_depth: Option<u32>,
            max_rows: Option<u32>,
        ) {
            assert_eq!(id == items.meta_root(), depth == 0);

            if max_depth.is_some_and(|max_depth| depth > max_depth) {
                return;
            }

            if max_rows.is_some_and(|max_rows| *row > max_rows) {
                return;
            }

            if depth > 0 {
                add_text_item(items, depth, id, table);
            }

            if let Some(children) = items.dominator_tree().get(&id) {
                let mut children = children.to_vec();
                children.sort_by(|a, b| items.retained_size(*b).cmp(&items.retained_size(*a)));
                for child in children {
                    *row += 1;
                    recursive_add_rows(
                        table,
                        items,
                        depth + 1,
                        &mut row,
                        child,
                        max_depth,
                        max_rows,
                    );
                }
            }
        }

        for &id in &self.item_ids {
            let start_depth = if id == self.items.meta_root() { 0 } else { 1 };
            recursive_add_rows(&mut table, self.items, start_depth, &mut row, id, max_depth, max_rows);
        }

        if let Some(UnreachableItemsSummary {
            count,
            size,
            size_percent,
        }) = self.unreachable_items_summary
        {
            table.add_row(vec![
                size.to_string(),
                format!("{:.2}%", size_percent),
                format!("[{} Unreachable Items]", count),
            ]);
        }

        write!(dest, "{}", &table)?;
        Ok(())
    }

    #[cfg(feature = "emit_json")]
    fn emit_json(&self, dest: impl io::Write) -> anyhow::Result<()> {
        fn recursive_item(id: ir::Id, items: &ir::Items) -> serde_json::Value {
            let item = &items[id];

            let shallow_size = item.size();
            let shallow_size_percent = f64::from(shallow_size) / f64::from(items.size()) * 100.0;

            let retained_size = items.retained_size(id);
            let retained_size_percent = f64::from(retained_size) / f64::from(items.size()) * 100.0;

            let mut obj = serde_json::Map::new();
            obj.insert("name".into(), item.name().into());
            obj.insert("shallow_size".into(), shallow_size.into());
            obj.insert("shallow_size_percent".into(), shallow_size_percent.into());
            obj.insert("retained_size".into(), retained_size.into());
            obj.insert("retained_size_percent".into(), retained_size_percent.into());

            if let Some(children) = items.dominator_tree().get(&id) {
                let mut children = children.to_vec();
                children.sort_by(|a, b| items.retained_size(*b).cmp(&items.retained_size(*a)));

                obj.insert(
                    "children".into(),
                    children
                        .iter()
                        .map(|&id| recursive_item(id, items))
                        .collect(),
                );
            }

            obj.into()
        }

        let mut obj = serde_json::Map::new();

        obj.insert(
            "items".into(),
            self.item_ids
                .iter()
                .map(|&id| recursive_item(id, self.items))
                .collect(),
        );

        if let Some(UnreachableItemsSummary {
            count,
            size,
            size_percent,
        }) = self.unreachable_items_summary
        {
            obj.insert(
                "summary".into(),
                serde_json::json!({
                    "name": format!("[{} Unreachable Items]", count),
                    "retained_size": size,
                    "retained_size_percent": size_percent,
                }),
            );
        }

        serde_json::to_writer_pretty(dest, &obj)?;
        Ok(())
    }

    #[cfg(feature = "emit_csv")]
    fn emit_csv(&self, dest: impl io::Write) -> anyhow::Result<()> {
        fn recursive_add_children(
            items: &ir::Items,
            id: ir::Id,
            wtr: &mut csv::Writer<impl io::Write>,
        ) -> anyhow::Result<()> {
            add_csv_item(items, id, wtr)?;
            if let Some(children) = items.dominator_tree().get(&id) {
                let mut children = children.to_vec();
                children.sort_by(|a, b| items.retained_size(*b).cmp(&items.retained_size(*a)));
                for child in children {
                    recursive_add_children(items, child, wtr)?;
                }
            }
            Ok(())
        }

        let mut wtr = csv::Writer::from_writer(dest);
        recursive_add_children(self.items, self.items.meta_root(), &mut wtr)?;

        if let Some(UnreachableItemsSummary {
            count,
            size,
            size_percent,
        }) = self.unreachable_items_summary
        {
            let rc = CsvRecord {
                id: None,
                name: format!("[{} Unreachable Items]", count),
                shallow_size: size,
                shallow_size_percent: size_percent,
                retained_size: size,
                retained_size_percent: size_percent,
                immediate_dominator: None,
            };
            wtr.serialize(rc)?;
            wtr.flush()?;
        }

        Ok(())
    }
}

fn add_text_item(items: &ir::Items, depth: u32, id: ir::Id, table: &mut Table) {
    let item = &items[id];

    let size = items.retained_size(id);
    let size_percent = (f64::from(size)) / (f64::from(items.size())) * 100.0;

    let mut label = String::with_capacity(depth as usize * 4 + item.name().len() + "⤷ ".len());
    for _ in 2..depth {
        label.push_str("    ");
    }
    if depth != 1 {
        label.push_str("  ⤷ ");
    }
    label.push_str(item.name());

    table.add_row(vec![
        size.to_string(),
        format!("{:.2}%", size_percent),
        label,
    ]);
}

#[cfg(feature = "emit_csv")]
#[derive(serde::Serialize, Debug)]
#[serde(rename_all = "PascalCase")]
struct CsvRecord {
    pub id: Option<u64>,
    pub name: String,
    pub shallow_size: u32,
    pub shallow_size_percent: f64,
    pub retained_size: u32,
    pub retained_size_percent: f64,
    pub immediate_dominator: Option<u64>,
}

#[cfg(feature = "emit_csv")]
fn add_csv_item(
    items: &ir::Items,
    id: ir::Id,
    wtr: &mut csv::Writer<impl io::Write>,
) -> anyhow::Result<()> {
    let item = &items[id];
    let (shallow_size, shallow_size_percent) = (
        item.size(),
        f64::from(item.size()) / f64::from(items.size()) * 100.0,
    );
    let (retained_size, retained_size_percent) = (
        items.retained_size(id),
        f64::from(items.retained_size(id)) / f64::from(items.size()) * 100.0,
    );
    let idom = if let Some(idom) = items.immediate_dominators().get(&id) {
        idom.serializable()
    } else {
        id.serializable()
    };

    let rc = CsvRecord {
        id: Some(item.id().serializable()),
        name: item.name().to_string(),
        shallow_size,
        shallow_size_percent,
        retained_size,
        retained_size_percent,
        immediate_dominator: Some(idom),
    };

    wtr.serialize(rc)?;
    wtr.flush()?;
    Ok(())
}

/// Compute the dominator tree for the given IR graph.
pub fn dominators(items: &mut ir::Items, opts: Options) -> anyhow::Result<Report<'_>> {
    items.compute_dominator_tree();
    items.compute_dominators();
    items.compute_retained_sizes();
    items.compute_predecessors();

    let dominator_items = if opts.item_names.is_empty() {
        vec![items.meta_root()]
    } else if opts.using_regexps {
        let regexps = regex::RegexSet::new(&opts.item_names)?;
        let mut sorted_items: Vec<_> = items
            .iter()
            .filter(|item| regexps.is_match(&item.name()))
            .map(|item| item.id())
            .collect();
        sorted_items.sort_by_key(|id| -i64::from(items.retained_size(*id)));
        sorted_items
    } else {
        opts.item_names
            .iter()
            .filter_map(|name| items.get_item_by_name(name))
            .map(|item| item.id())
            .collect()
    };

    Ok(Report {
        item_ids: dominator_items,
        items,
        unreachable_items_summary: summarize_unreachable_items(items, opts),
    })
}

fn summarize_unreachable_items(
    items: &ir::Items,
    opts: Options,
) -> Option<UnreachableItemsSummary> {
    let (size, count) = garbage::get_unreachable_items(&items)
        .map(|item| item.size())
        .fold((0, 0), |(s, c), curr| (s + curr, c + 1));
    if opts.item_names.is_empty() && size > 0 {
        Some(UnreachableItemsSummary {
            count,
            size,
            size_percent: (f64::from(size)) / (f64::from(items.size())) * 100.0,
        })
    } else {
        None
    }
}
