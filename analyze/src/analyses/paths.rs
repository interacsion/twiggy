use std::{cmp, collections::BTreeSet, io, iter};

use twiggy_ir as ir;

use crate::{
    formats::table::{Align, Table},
    OutputFormat,
};

#[derive(Clone, Debug)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct Options {
    /// The functions to find call paths to.
    pub functions: Vec<String>,

    /// This direction of the path traversal.
    #[cfg_attr(feature = "clap", arg(long))]
    pub descending: bool,

    /// Whether or not `functions` should be treated as regular expressions.
    #[cfg_attr(feature = "clap", arg(long = "regex"))]
    pub using_regexps: bool,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct EmitOptions {
    /// The format the output should be written in.
    #[cfg_attr(feature = "clap", arg(short, long, default_value_t))]
    pub format: OutputFormat,

    /// The maximum depth to print the paths.
    #[cfg_attr(feature = "clap", arg(short = 'd', default_value = "10"))]
    pub max_depth: u32,

    /// The maximum number of paths, regardless of depth in the tree, to display.
    #[cfg_attr(feature = "clap", arg(short = 'r', default_value = "10"))]
    pub max_paths: u32,
}

#[derive(Clone, Debug)]
pub struct Report<'a> {
    entries: Vec<PathsEntry>,
    descending: bool,
    items: &'a ir::Items,
}

#[derive(PartialEq, Eq, Clone, Debug)]
struct PathsEntry {
    name: String,
    size: u32,
    children: Vec<PathsEntry>,
}

impl PartialOrd for PathsEntry {
    fn partial_cmp(&self, rhs: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(rhs))
    }
}

impl Ord for PathsEntry {
    fn cmp(&self, rhs: &Self) -> cmp::Ordering {
        rhs.size.cmp(&self.size).then(self.name.cmp(&rhs.name))
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
        /// This structure represents a row in the emitted text table. Size, and size
        /// percentage are only shown for the top-most rows.
        struct TableRow {
            pub size: Option<u32>,
            pub size_percent: Option<f64>,
            pub name: String,
        }

        /// Process a given path entry, and return an iterator of table rows,
        /// representing its related call paths, according to the given options.
        fn process_entry<'a>(
            entry: &'a PathsEntry,
            depth: u32,
            items: &'a ir::Items,
            opts: &'a EmitOptions,
            descending: bool,
        ) -> Box<dyn Iterator<Item = TableRow> + 'a> {
            // Get the row's name and size columns using the current depth.
            let name = get_indented_name(&entry.name, depth, descending);
            let (size, size_percent) = if depth == 0 {
                (
                    Some(entry.size),
                    Some(f64::from(entry.size) / f64::from(items.size()) * 100.0),
                )
            } else {
                (None, None)
            };

            // Create an iterator containing the current entry's table row.
            let row_iter = iter::once(TableRow {
                size,
                size_percent,
                name,
            });

            if depth < opts.max_depth {
                // Process each child entry, and chain together the resulting iterators.
                let children_iter = entry
                    .children
                    .iter()
                    .take(opts.max_paths as usize)
                    .flat_map(move |child_entry| {
                        process_entry(child_entry, depth + 1, items, &opts, descending)
                    });
                Box::new(row_iter.chain(children_iter))
            } else if depth == opts.max_depth {
                // TODO: Create a summary row, and chain it to the row iterator.
                Box::new(row_iter)
            } else {
                // If we are beyond the maximum depth, return an empty iterator.
                Box::new(iter::empty())
            }
        }

        /// Given the name of an item, its depth, and the traversal direction,
        /// return an indented version of the name for its corresponding table row.
        fn get_indented_name(name: &str, depth: u32, descending: bool) -> String {
            (1..depth)
                .map(|_| "    ")
                .chain(iter::once(if depth > 0 && descending {
                    "  ↳ "
                } else if depth > 0 {
                    "  ⬑ "
                } else {
                    ""
                }))
                .chain(iter::once(name))
                .fold(
                    String::with_capacity(depth as usize * 4 + name.len()),
                    |mut res, s| {
                        res.push_str(s);
                        res
                    },
                )
        }

        // Flat map each entry and its children into a sequence of table rows.
        // Convert these `TableRow` objects into vectors of strings, and add
        // each of these to the table before writing the table to `dest`.
        let table = self
            .entries
            .iter()
            .flat_map(|entry| process_entry(entry, 0, self.items, &opts, self.descending))
            .map(
                |TableRow {
                     size,
                     size_percent,
                     name,
                 }| {
                    vec![
                        size.map(|size| size.to_string())
                            .unwrap_or_else(String::new),
                        size_percent
                            .map(|size_percent| format!("{:.2}%", size_percent))
                            .unwrap_or_else(String::new),
                        name,
                    ]
                },
            )
            .fold(
                Table::with_header(vec![
                    (Align::Right, "Shallow Bytes".to_string()),
                    (Align::Right, "Shallow %".to_string()),
                    (Align::Left, "Retaining Paths".to_string()),
                ]),
                |mut table, row| {
                    table.add_row(row);
                    table
                },
            );

        write!(dest, "{}", table)?;
        Ok(())
    }

    #[cfg(feature = "emit_json")]
    fn emit_json(&self, opts: EmitOptions, dest: impl io::Write) -> anyhow::Result<()> {
        // Process a paths entry, by adding its name and size to the given JSON object.
        fn process_entry(
            entry: &PathsEntry,
            depth: u32,
            items: &ir::Items,
            opts: &EmitOptions,
        ) -> serde_json::Value {
            let callers = if depth < opts.max_depth {
                entry
                    .children
                    .iter()
                    .take(opts.max_paths as usize)
                    .map(|child| process_entry(child, depth + 1, items, opts))
                    .collect()
            } else {
                vec![]
            };

            serde_json::json!({
                "name": entry.name,
                "shallow_size": entry.size,
                "shallow_size_percent":  f64::from(entry.size) / f64::from(items.size()) * 100.0,
                "callers": callers
            })
        }

        serde_json::to_writer_pretty(
            dest,
            &self
                .entries
                .iter()
                .map(|entry| process_entry(entry, 0, self.items, &opts))
                .collect::<Vec<_>>(),
        )?;
        Ok(())
    }

    #[cfg(feature = "emit_csv")]
    fn emit_csv(&self, opts: EmitOptions, dest: impl io::Write) -> anyhow::Result<()> {
        /// This structure represents a row in the CSV output.
        #[derive(serde::Serialize, Debug)]
        #[serde(rename_all = "PascalCase")]
        struct CsvRecord {
            pub name: String,
            pub shallow_size: u32,
            pub shallow_size_percent: f64,
            pub path: Option<String>,
        }

        // Process a given entry and its children, returning an iterator of CSV records.
        fn process_entry<'a>(
            entry: &'a PathsEntry,
            depth: u32,
            paths: usize,
            items: &'a ir::Items,
            opts: &'a EmitOptions,
        ) -> Box<dyn Iterator<Item = CsvRecord> + 'a> {
            let name = entry.name.clone();
            let shallow_size = entry.size;
            let shallow_size_percent = f64::from(entry.size) / f64::from(items.size()) * 100.0;
            let path = get_path(entry);

            // Create an iterator containing the current entry's CSV record.
            let record_iter = iter::once(CsvRecord {
                name,
                shallow_size,
                shallow_size_percent,
                path,
            });

            if depth < opts.max_depth {
                // Process each child entry, and chain together the resulting iterators.
                let children_iter =
                    entry
                        .children
                        .iter()
                        .take(paths)
                        .flat_map(move |child_entry| {
                            process_entry(child_entry, depth + 1, paths, items, opts)
                        });
                Box::new(record_iter.chain(children_iter))
            } else if depth == opts.max_depth {
                // Create a summary row, and chain it to the row iterator.
                Box::new(record_iter)
            } else {
                // If we are beyond the maximum depth, return an empty iterator.
                Box::new(iter::empty())
            }
        }

        // Given a path entry, return the value for its corresponding CsvRecord's `path` field.
        fn get_path(entry: &PathsEntry) -> Option<String> {
            if entry.children.is_empty() {
                None
            } else {
                Some(
                    entry
                        .children
                        .iter()
                        .map(|child| child.name.as_str())
                        .chain(iter::once(entry.name.as_str()))
                        .collect::<Vec<_>>()
                        .join(" -> "),
                )
            }
        }

        // First, initialize a CSV writer. Then, flat map each entry and its
        // children into a sequence of `CsvRecord` objects. Send each record
        // to the CSV writer to be serialized.
        let mut wtr = csv::Writer::from_writer(dest);
        for record in self
            .entries
            .iter()
            .flat_map(|entry| process_entry(entry, 0, opts.max_paths as usize, self.items, &opts))
        {
            wtr.serialize(record)?;
            wtr.flush()?;
        }

        Ok(())
    }
}

/// Find all retaining paths for the given items.
pub fn paths(items: &mut ir::Items, opts: Options) -> anyhow::Result<Report<'_>> {
    // The predecessor tree only needs to be computed if we are ascending
    // through the retaining paths.
    if !opts.descending {
        items.compute_predecessors();
    }

    // Initialize the collection of Id values whose retaining paths we will emit.
    let opts = opts.clone();
    let entries = get_starting_positions(items, &opts)?
        .iter()
        .map(|id| create_entry(*id, items, &opts, &mut BTreeSet::new()))
        .collect();

    Ok(Report {
        entries,
        descending: opts.descending,
        items,
    })
}

/// This helper function is used to collect the `ir::Id` values for the top-most
/// path entries for the `Paths` object, based on the given options.
fn get_starting_positions(items: &ir::Items, opts: &Options) -> anyhow::Result<Vec<ir::Id>> {
    // Collect Id's if no arguments are given and we are ascending the retaining paths.
    let get_functions_default = || -> Vec<ir::Id> {
        let mut sorted_items = items
            .iter()
            .filter(|item| item.id() != items.meta_root())
            .collect::<Vec<_>>();
        sorted_items.sort_by(|a, b| b.size().cmp(&a.size()));
        sorted_items.iter().map(|item| item.id()).collect()
    };

    // Collect Id's if no arguments are given and we are descending the retaining paths.
    let get_functions_default_desc = || -> Vec<ir::Id> {
        let mut roots = items
            .neighbors(items.meta_root())
            .map(|id| &items[id])
            .collect::<Vec<_>>();
        roots.sort_by(|a, b| b.size().cmp(&a.size()));
        roots.into_iter().map(|item| item.id()).collect()
    };

    // Collect Id's if arguments were given that should be used as regular expressions.
    let get_regexp_matches = || -> anyhow::Result<Vec<ir::Id>> {
        let regexps = regex::RegexSet::new(&opts.functions)?;
        let matches = items
            .iter()
            .filter(|item| regexps.is_match(item.name()))
            .map(|item| item.id())
            .collect();
        Ok(matches)
    };

    // Collect Id's if arguments were given that should be used as exact names.
    let get_exact_matches = || -> Vec<ir::Id> {
        opts.functions
            .iter()
            .filter_map(|s| items.get_item_by_name(s))
            .map(|item| item.id())
            .collect()
    };

    // Collect the starting positions based on the relevant options given.
    // If arguments were given, search for matches depending on whether or
    // not these should be treated as regular expressions. Otherwise, collect
    // the starting positions based on the direction we will be traversing.
    let args_given = !opts.functions.is_empty();
    let using_regexps = opts.using_regexps;
    let descending = opts.descending;
    let res = match (args_given, using_regexps, descending) {
        (true, true, _) => get_regexp_matches()?,
        (true, false, _) => get_exact_matches(),
        (false, _, true) => get_functions_default_desc(),
        (false, _, false) => get_functions_default(),
    };

    Ok(res)
}

/// Create a `PathsEntry` object for the given item.
fn create_entry(
    id: ir::Id,
    items: &ir::Items,
    opts: &Options,
    seen: &mut BTreeSet<ir::Id>,
) -> PathsEntry {
    // Determine the item's name and size.
    let item = &items[id];
    let name = item.name().to_string();
    let size = item.size();

    // Collect the `ir::Id` values of this entry's children, depending on
    // whether we are ascending or descending the IR-tree.
    let children_ids: Vec<ir::Id> = if opts.descending {
        items
            .neighbors(id)
            .map(|id| id as ir::Id)
            .filter(|id| !seen.contains(id))
            .filter(|&id| id != items.meta_root())
            .collect()
    } else {
        items
            .predecessors(id)
            .map(|id| id as ir::Id)
            .filter(|id| !seen.contains(id))
            .filter(|&id| id != items.meta_root())
            .collect()
    };

    // Temporarily add the current item to the set of discovered nodes, and
    // create an entry for each child. Collect these into a `children` vector.
    seen.insert(id);
    let children = children_ids
        .into_iter()
        .map(|id| create_entry(id, items, opts, seen))
        .collect();
    seen.remove(&id);

    PathsEntry {
        name,
        size,
        children,
    }
}
