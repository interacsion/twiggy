use std::collections::BTreeSet;
use std::io;

use petgraph::visit::Walker;
use twiggy_ir as ir;

use crate::{
    formats::table::{Align, Table},
    OutputFormat,
};

#[derive(Clone, Debug)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct Options {
    /// Show data segments rather than summarizing them in a single line.
    #[cfg_attr(feature = "clap", arg(long))]
    pub show_data_segments: bool,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct EmitOptions {
    /// The format the output should be written in.
    #[cfg_attr(feature = "clap", arg(short, long, default_value_t))]
    pub format: OutputFormat,

    /// The maximum number of items to display.
    #[cfg_attr(feature = "clap", arg(short = 'n', default_value = "20"))]
    pub max_items: u32,
}

#[derive(Clone, Debug)]
pub struct Report<'a> {
    unreachable_items: Vec<ir::Id>,
    unreachable_data: Vec<ir::Id>,
    items: &'a ir::Items,
}

impl Report<'_> {
    pub fn emit(&self, opts: EmitOptions, dest: impl io::Write) -> anyhow::Result<()> {
        match opts.format {
            OutputFormat::Text => self.emit_text(opts.max_items, dest),
            #[cfg(feature = "emit_json")]
            OutputFormat::Json => self.emit_json(opts.max_items, dest),
            #[cfg(feature = "emit_csv")]
            OutputFormat::Csv => self.emit_csv(opts.max_items, dest),
        }
    }

    fn emit_text(&self, max_items: u32, mut dest: impl io::Write) -> anyhow::Result<()> {
        let mut table = Table::with_header(vec![
            (Align::Right, "Bytes".to_string()),
            (Align::Right, "Size %".to_string()),
            (Align::Left, "Garbage Item".to_string()),
        ]);
        let items_iter = self.unreachable_items.iter().map(|id| &self.items[*id]);

        for item in items_iter.clone().take(max_items as usize) {
            let size = item.size();
            let size_percent = (f64::from(size)) / (f64::from(self.items.size())) * 100.0;
            table.add_row(vec![
                size.to_string(),
                format!("{:.2}%", size_percent),
                item.name().to_string(),
            ]);
        }

        match items_iter
            .clone()
            .skip(max_items as usize)
            .fold((0, 0), |(size, cnt), item| (size + item.size(), cnt + 1))
        {
            (size, cnt) if cnt > 0 => {
                let size_percent = f64::from(size) / f64::from(self.items.size()) * 100.0;
                table.add_row(vec![
                    size.to_string(),
                    format!("{:.2}%", size_percent),
                    format!("... and {} more", cnt),
                ]);
            }
            _ => {}
        }

        let total_size: u32 = items_iter.map(|item| item.size()).sum();
        let total_percent = (f64::from(total_size)) / (f64::from(self.items.size())) * 100.0;
        table.add_row(vec![
            total_size.to_string(),
            format!("{:.2}%", total_percent),
            format!("Σ [{} Total Rows]", self.unreachable_items.len()),
        ]);

        if !self.unreachable_data.is_empty() {
            let total_size: u32 = self
                .unreachable_data
                .iter()
                .map(|&id| self.items[id].size())
                .sum();
            let size_percent = f64::from(total_size) / f64::from(self.items.size()) * 100.0;
            table.add_row(vec![
                total_size.to_string(),
                format!("{:.2}%", size_percent),
                format!(
                    "{} potential false-positive data segments",
                    self.unreachable_data.len()
                ),
            ]);
        }

        write!(dest, "{}", &table)?;
        Ok(())
    }

    #[cfg(feature = "emit_json")]
    fn emit_json(&self, max_items: u32, dest: impl io::Write) -> anyhow::Result<()> {
        let mut objs = Vec::new();

        for &id in self.unreachable_items.iter().take(max_items as usize) {
            let item = &self.items[id];

            let size = item.size();
            let size_percent = (f64::from(size)) / (f64::from(self.items.size())) * 100.0;

            objs.push(serde_json::json!({
                "name": item.name(),
                "bytes": size,
                "size_Percent": size_percent,
            }));
        }

        let (total_size, total_cnt) = self
            .unreachable_items
            .iter()
            .skip(max_items as usize)
            .map(|id| &self.items[*id])
            .fold((0, 0), |(size, cnt), item| (size + item.size(), cnt + 1));

        if total_cnt > 0 {
            let name = format!("... and {} more", total_cnt);
            let total_size_percent =
                (f64::from(total_size)) / (f64::from(self.items.size())) * 100.0;

            objs.push(serde_json::json!({
                "name": name,
                "bytes": total_size,
                "size_percent": total_size_percent,
            }));
        }

        let total_name = format!("Σ [{} Total Rows]", self.unreachable_items.len());
        let total_size: u32 = self
            .unreachable_items
            .iter()
            .map(|&id| self.items[id].size())
            .sum();
        let total_size_percent = (f64::from(total_size)) / (f64::from(self.items.size())) * 100.0;

        objs.push(serde_json::json!({
            "name": total_name,
            "bytes": total_size,
            "size_percent": total_size_percent,
        }));

        if !self.unreachable_data.is_empty() {
            let name = format!(
                "{} potential false-positive data segments",
                self.unreachable_data.len()
            );
            let size: u32 = self
                .unreachable_data
                .iter()
                .map(|&id| self.items[id].size())
                .sum();
            let size_percent = f64::from(size) / f64::from(self.items.size()) * 100.0;

            objs.push(serde_json::json!({
                "name": name,
                "bytes": size,
                "size_percent": size_percent,
            }));
        }

        serde_json::to_writer_pretty(dest, &objs)?;
        Ok(())
    }

    #[cfg(feature = "emit_csv")]
    fn emit_csv(&self, _max_items: u32, _dest: impl io::Write) -> anyhow::Result<()> {
        unimplemented!()
    }
}

/// Find items that are not transitively referenced by any exports or public functions.
pub fn garbage(items: &mut ir::Items, opts: Options) -> anyhow::Result<Report<'_>> {
    let mut unreachable_items = get_unreachable_items(&items).collect::<Vec<_>>();
    unreachable_items.sort_by(|a, b| b.size().cmp(&a.size()));

    // Split the items into two categories if necessary
    let (data_segments, items_non_data) = if opts.show_data_segments {
        (
            vec![],
            unreachable_items.iter().map(|item| item.id()).collect(),
        )
    } else {
        (
            unreachable_items
                .iter()
                .filter(|item| item.kind().is_data())
                .map(|item| item.id())
                .collect(),
            unreachable_items
                .iter()
                .filter(|item| !item.kind().is_data())
                .map(|item| item.id())
                .collect(),
        )
    };

    Ok(Report {
        unreachable_items: items_non_data,
        unreachable_data: data_segments,
        items,
    })
}

pub(crate) fn get_unreachable_items(items: &ir::Items) -> impl Iterator<Item = &ir::Item> {
    let reachable_items = petgraph::visit::Dfs::new(items, items.meta_root())
        .iter(&items)
        .collect::<BTreeSet<ir::Id>>();
    items
        .iter()
        .filter(move |item| !reachable_items.contains(&item.id()))
}
