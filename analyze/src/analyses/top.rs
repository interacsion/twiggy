use std::io;

use anyhow::anyhow;
use twiggy_ir as ir;

use crate::{
    formats::table::{Align, Table},
    OutputFormat,
};

#[derive(Clone, Debug)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct Options {
    /// Display retaining paths.
    #[cfg_attr(feature = "clap", arg(short, long))]
    pub retaining_paths: bool,

    /// Sort list by retained size, rather than shallow size.
    #[cfg_attr(feature = "clap", arg(long))]
    pub retained: bool,
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
    top_items: Vec<ir::Id>,
    items: &'a ir::Items,
    retained: bool,
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
        // A struct used to represent a row in the table that will be emitted.
        struct TableRow {
            name: String,
            size: u32,
            size_percent: f64,
        }

        // Helper function used to process an item, and return a struct
        // representing a row containing its size and name.
        fn process_item(id: ir::Id, items: &ir::Items, retained: bool) -> TableRow {
            let item = &items[id];

            let size = if retained {
                items.retained_size(id)
            } else {
                item.size()
            };

            let size_percent = (f64::from(size)) / (f64::from(items.size())) * 100.0;

            TableRow {
                name: item.name().into(),
                size,
                size_percent,
            }
        }

        // Helper function used to summnarize a sequence of table rows. This is
        // used to generate the remaining summary and total rows. Returns a tuple
        // containing the total size, total size percentage, and number of items.
        fn summarize_rows(rows: impl Iterator<Item = TableRow>) -> (u32, f64, u32) {
            rows.fold(
                (0, 0.0, 0),
                |(total_size, total_percent, remaining_count), row| {
                    (
                        total_size + row.size,
                        total_percent + row.size_percent,
                        remaining_count + 1,
                    )
                },
            )
        }

        let sort_label = if self.retained { "Retained" } else { "Shallow" };

        let mut table = Table::with_header(vec![
            (Align::Right, format!("{} Bytes", sort_label)),
            (Align::Right, format!("{} %", sort_label)),
            (Align::Left, "Item".into()),
        ]);

        // Process the number of items specified, and add them to the table.
        self.top_items
            .iter()
            .take(max_items as usize)
            .map(|&id| process_item(id, self.items, self.retained))
            .for_each(|row| {
                table.add_row(vec![
                    row.size.to_string(),
                    format!("{:.2}%", row.size_percent),
                    row.name,
                ])
            });

        // Find the summary statistics by processing the remaining items.
        let remaining_rows = self
            .top_items
            .iter()
            .skip(max_items as usize)
            .map(|&id| process_item(id, self.items, self.retained));

        let (rem_size, rem_size_percent, rem_count) = summarize_rows(remaining_rows);

        // If there were items remaining, add a summary row to the table.
        if rem_count > 0 {
            let rem_name_col = format!("... and {} more.", rem_count);

            let (rem_size_col, rem_size_percent_col) = if self.retained {
                ("...".into(), "...".into())
            } else {
                (rem_size.to_string(), format!("{:.2}%", rem_size_percent))
            };

            table.add_row(vec![rem_size_col, rem_size_percent_col, rem_name_col]);
        }

        // Add a row containing the totals to the table.
        let all_rows = self
            .top_items
            .iter()
            .map(|&id| process_item(id, self.items, self.retained));

        let (total_size, total_size_percent, total_count) = summarize_rows(all_rows);

        let (total_size_col, total_size_percent_col) = if self.retained {
            ("...".into(), "...".into())
        } else {
            (
                total_size.to_string(),
                format!("{:.2}%", total_size_percent),
            )
        };

        let total_name_col = format!("Σ [{} Total Rows]", total_count);

        table.add_row(vec![total_size_col, total_size_percent_col, total_name_col]);

        // Write the generated table out to the destination and return.
        write!(dest, "{}", &table)?;
        Ok(())
    }

    #[cfg(feature = "emit_json")]
    fn emit_json(&self, max_items: u32, dest: impl io::Write) -> anyhow::Result<()> {
        serde_json::to_writer_pretty(
            dest,
            &self
                .top_items
                .iter()
                .take(max_items as usize)
                .map(|&id| {
                    let item = &self.items[id];

                    let mut obj = serde_json::Map::new();
                    obj.insert("name".into(), item.name().into());

                    let size = item.size();
                    let size_percent = f64::from(size) / f64::from(self.items.size()) * 100.0;
                    obj.insert("shallow_size".into(), size.into());
                    obj.insert("shallow_size_percent".into(), size_percent.into());

                    if self.retained {
                        let size = self.items.retained_size(id);
                        let size_percent = f64::from(size) / f64::from(self.items.size()) * 100.0;
                        obj.insert("retained_size".into(), size.into());
                        obj.insert("retained_size_percent".into(), size_percent.into());
                    }
                })
                .collect::<Vec<_>>(),
        )?;
        Ok(())
    }

    #[cfg(feature = "emit_csv")]
    fn emit_csv(&self, max_items: u32, dest: impl io::Write) -> anyhow::Result<()> {
        let mut writer = csv::Writer::from_writer(dest);

        #[derive(serde::Serialize)]
        #[serde(rename_all = "PascalCase")]
        struct Record {
            name: String,
            shallow_size: u32,
            shallow_size_percent: f64,
            retained_size: Option<u32>,
            retained_size_percent: Option<f64>,
        }

        for &id in self.top_items.iter().take(max_items as usize) {
            let item = &self.items[id];

            let (shallow_size, shallow_size_percent) = {
                let size = item.size();
                let size_percent = f64::from(size) / f64::from(self.items.size()) * 100.0;
                (size, size_percent)
            };

            let (retained_size, retained_size_percent) = if self.retained {
                let size = self.items.retained_size(id);
                let size_percent = f64::from(size) / f64::from(self.items.size()) * 100.0;
                (Some(size), Some(size_percent))
            } else {
                (None, None)
            };

            writer.serialize(Record {
                name: item.name().into(),
                shallow_size,
                shallow_size_percent,
                retained_size,
                retained_size_percent,
            })?;

            writer.flush()?;
        }

        Ok(())
    }
}

/// Run the `top` analysis on the given IR items.
pub fn top(items: &mut ir::Items, opts: Options) -> anyhow::Result<Report<'_>> {
    if opts.retaining_paths {
        return Err(anyhow!("retaining paths are not yet implemented",));
    }

    if opts.retained {
        items.compute_retained_sizes();
    }

    let mut top_items: Vec<_> = items
        .iter()
        .filter(|item| item.id() != items.meta_root())
        .collect();

    top_items.sort_by(|a, b| {
        if opts.retained {
            items
                .retained_size(b.id())
                .cmp(&items.retained_size(a.id()))
        } else {
            b.size().cmp(&a.size())
        }
    });

    let top_items: Vec<_> = top_items.into_iter().map(|i| i.id()).collect();

    Ok(Report {
        top_items,
        items,
        retained: opts.retained,
    })
}
