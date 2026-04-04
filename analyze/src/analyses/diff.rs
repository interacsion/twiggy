use std::collections::{HashMap, HashSet};
use std::{cmp, io};

use anyhow::anyhow;
use twiggy_ir as ir;

use crate::formats::table::{Align, Table};
use crate::OutputFormat;

#[derive(Clone, Debug)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct Options {
    /// The name of the item(s) whose diff should be printed.
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

    /// The maximum number of items to display.
    #[cfg_attr(feature = "clap", arg(short = 'n', default_value = "20"))]
    pub max_items: u32,
}

#[derive(Clone, Debug)]
pub struct Report<'a> {
    deltas: Vec<DiffEntry>,
    item_names: Vec<String>,
    new_items: &'a ir::Items,
    old_items: &'a ir::Items,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct DiffEntry {
    name: String,
    delta: i64,
}

impl PartialOrd for DiffEntry {
    fn partial_cmp(&self, rhs: &DiffEntry) -> Option<cmp::Ordering> {
        Some(self.cmp(rhs))
    }
}

impl Ord for DiffEntry {
    fn cmp(&self, rhs: &DiffEntry) -> cmp::Ordering {
        rhs.delta
            .abs()
            .cmp(&self.delta.abs())
            .then(self.name.cmp(&rhs.name))
    }
}

#[cfg(feature = "emit_csv")]
impl serde::Serialize for DiffEntry {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("DiffEntry", 2)?;
        state.serialize_field("DeltaBytes", &format!("{:+}", self.delta))?;
        state.serialize_field("Item", &self.name)?;
        state.end()
    }
}

impl Report<'_> {
    pub fn emit(&self, opts: EmitOptions, dest: impl io::Write) -> anyhow::Result<()> {
        match opts.format {
            OutputFormat::Text => self.emit_text(opts, dest),
            #[cfg(feature = "emit_json")]
            OutputFormat::Json => self.emit_json(opts, dest),
            #[cfg(feature = "emit_csv")]
            OutputFormat::Csv => self.emit_csv(opts, dest),
        }
    }

    fn emit_text(&self, opts: EmitOptions, mut dest: impl io::Write) -> anyhow::Result<()> {
        let mut table = Table::with_header(vec![
            (Align::Right, "Delta Bytes".into()),
            (Align::Left, "Item".to_string()),
        ]);

        self.truncate_deltas(opts.max_items)
            .iter()
            .map(|entry| vec![format!("{:+}", entry.delta), entry.name.clone()])
            .for_each(|row| table.add_row(row));

        write!(dest, "{}", &table)?;
        Ok(())
    }

    #[cfg(feature = "emit_json")]
    fn emit_json(&self, opts: EmitOptions, dest: impl io::Write) -> anyhow::Result<()> {
        serde_json::to_writer_pretty(
            dest,
            &self
                .truncate_deltas(opts.max_items)
                .iter()
                .map(|entry| {
                    serde_json::json!({
                        "delta_bytes": entry.delta as f64,
                        "name": entry.name,
                    })
                })
                .collect::<Vec<_>>(),
        )?;
        Ok(())
    }

    #[cfg(feature = "emit_csv")]
    fn emit_csv(&self, opts: EmitOptions, dest: impl io::Write) -> anyhow::Result<()> {
        let mut wtr = csv::Writer::from_writer(dest);

        for entry in &self.truncate_deltas(opts.max_items) {
            wtr.serialize(entry)?;
            wtr.flush()?;
        }

        Ok(())
    }

    fn truncate_deltas(&self, max_items: u32) -> Vec<DiffEntry> {
        let mut deltas = self.deltas.clone();

        // Create an entry to summarize the diff rows that will be truncated.
        let (rem_cnt, rem_delta): (u32, i64) = deltas
            .iter()
            .skip(max_items as usize)
            .fold((0, 0), |(cnt, rem_delta), DiffEntry { delta, .. }| {
                (cnt + 1, rem_delta + delta)
            });
        let remaining = DiffEntry {
            name: format!("... and {} more.", rem_cnt),
            delta: rem_delta,
        };

        // Create a `DiffEntry` representing the net change, and total row count.
        // If specifying arguments were not given, calculate the total net changes,
        // otherwise find the total values only for items in the the deltas collection.
        let (total_cnt, total_delta) = if self.item_names.is_empty() {
            (
                deltas.len(),
                i64::from(self.new_items.size()) - i64::from(self.old_items.size()),
            )
        } else {
            deltas
                .iter()
                .fold((0, 0), |(cnt, rem_delta), DiffEntry { delta, .. }| {
                    (cnt + 1, rem_delta + delta)
                })
        };
        let total = DiffEntry {
            name: format!("Σ [{} Total Rows]", total_cnt),
            delta: total_delta,
        };

        // Now that the 'remaining' and 'total' summary entries have been created,
        // truncate the vector of deltas before we box up the result, and push
        // the remaining and total rows to the deltas vector.
        deltas.truncate(max_items as usize);
        if rem_cnt > 0 {
            deltas.push(remaining);
        }
        deltas.push(total);

        deltas
    }
}

/// Compute the diff between two sets of items.
pub fn diff<'a>(
    old_items: &'a mut ir::Items,
    new_items: &'a mut ir::Items,
    opts: Options,
) -> anyhow::Result<Report<'a>> {
    // Given a set of items, create a HashMap of the items' names and sizes.
    fn get_names_and_sizes(items: &ir::Items) -> HashMap<&str, i64> {
        items
            .iter()
            .map(|item| (item.name(), i64::from(item.size())))
            .collect()
    }

    // Collect the names and sizes of the items in the old and new collections.
    let old_sizes = get_names_and_sizes(old_items);
    let new_sizes = get_names_and_sizes(new_items);

    // Given an item name, create a `DiffEntry` object representing the
    // change in size, or an error if the name could not be found in
    // either of the item collections.
    let get_item_delta = |name: String| -> anyhow::Result<DiffEntry> {
        let old_size = old_sizes.get::<str>(&name);
        let new_size = new_sizes.get::<str>(&name);
        let delta: i64 = match (old_size, new_size) {
            (Some(old_size), Some(new_size)) => new_size - old_size,
            (Some(old_size), None) => -old_size,
            (None, Some(new_size)) => *new_size,
            (None, None) => {
                return Err(anyhow!("Could not find item with name `{}`", name));
            }
        };
        Ok(DiffEntry { name, delta })
    };

    // Given a result returned by `get_item_delta`, return false if the result
    // represents an unchanged item. Ignore errors, these are handled separately.
    let unchanged_items_filter = |res: &anyhow::Result<DiffEntry>| -> bool {
        !matches!(res, Ok(DiffEntry { delta: 0, .. }))
    };

    // Create a set of item names from the new and old item collections.
    let names = old_sizes
        .keys()
        .chain(new_sizes.keys())
        .map(|k| k.to_string());

    // If arguments were given to the command, we should filter out items that
    // do not match any of the given names or expressions.
    let names: HashSet<String> = if !opts.item_names.is_empty() {
        if opts.using_regexps {
            let regexps = regex::RegexSet::new(&opts.item_names)?;
            names.filter(|name| regexps.is_match(name)).collect()
        } else {
            let item_names = opts.item_names.iter().collect::<HashSet<_>>();
            names.filter(|name| item_names.contains(&name)).collect()
        }
    } else {
        names.collect()
    };

    // Iterate through the set of item names, and use the closure above to map
    // each item into a `DiffEntry` object. Then, sort the collection.
    let mut deltas = names
        .into_iter()
        .map(get_item_delta)
        .filter(unchanged_items_filter)
        .collect::<anyhow::Result<Vec<_>>>()?;
    deltas.sort();

    // Return the results so that they can be emitted.
    Ok(Report {
        deltas,
        item_names: opts.item_names,
        old_items,
        new_items,
    })
}
