use std::collections::{BTreeMap, BTreeSet};
use std::{cmp, io, iter};

use regex;
use twiggy_ir as ir;

use crate::formats::table::{Align, Table};
use crate::OutputFormat;

#[derive(Clone, Debug)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct Options {
    /// The names of the generic functions whose monomorphizations
    /// should be printed.
    pub functions: Vec<String>,

    /// Hide individual monomorphizations and only show the generic functions.
    #[cfg_attr(feature = "clap", arg(long))]
    pub only_generics: bool,

    /// The maximum number of generics to list.
    #[cfg_attr(feature = "clap", arg(short, long, default_value = "10"))]
    pub max_generics: u32,

    /// The maximum number of individual monomorphizations to list for each
    /// listed generic function.
    #[cfg_attr(feature = "clap", arg(short = 'n', long, default_value = "10"))]
    pub max_monos: u32,

    /// List all generics and all of their individual monomorphizations.
    /// If combined with -g then monomorphizations are hidden.
    /// Overrides -m <max_generics> and -n <max_monos>
    #[cfg_attr(feature = "clap", arg(short, long = "all"))]
    pub all_generics_and_monos: bool,

    /// List all generics. Overrides -m <max_generics>
    #[cfg_attr(feature = "clap", arg(long))]
    pub all_generics: bool,

    /// List all individual monomorphizations for each listed generic
    /// function. Overrides -n <max_monos>
    #[cfg_attr(feature = "clap", arg(long))]
    pub all_monos: bool,

    /// Whether or not `names` should be treated as regular expressions.
    #[cfg_attr(feature = "clap", arg(long = "regex"))]
    pub using_regexps: bool,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "clap", derive(clap::Args))]
pub struct EmitOptions {
    /// The format the output should be written in.
    #[cfg_attr(feature = "clap", arg(short, long, default_value_t))]
    pub format: OutputFormat,
}

pub struct Report<'a> {
    monos: Vec<MonosEntry>,
    items: &'a ir::Items,
}

#[derive(Debug, PartialEq, Eq)]
struct MonosEntry {
    pub name: String,
    pub insts: Vec<(String, u32)>,
    pub size: u32,
    pub bloat: u32,
}

impl PartialOrd for MonosEntry {
    fn partial_cmp(&self, rhs: &MonosEntry) -> Option<std::cmp::Ordering> {
        Some(self.cmp(rhs))
    }
}

impl Ord for MonosEntry {
    fn cmp(&self, rhs: &MonosEntry) -> std::cmp::Ordering {
        rhs.bloat
            .cmp(&self.bloat)
            .then(rhs.size.cmp(&self.size))
            .then(self.insts.cmp(&rhs.insts))
            .then(self.name.cmp(&rhs.name))
    }
}

impl Report<'_> {
    pub fn emit(&self, opts: EmitOptions, dest: impl io::Write) -> anyhow::Result<()> {
        match opts.format {
            OutputFormat::Text => self.emit_text(dest),
            #[cfg(feature = "emit_json")]
            OutputFormat::Json => self.emit_json(dest),
            #[cfg(feature = "emit_csv")]
            OutputFormat::Csv => self.emit_csv(dest),
        }
    }

    fn emit_text(&self, mut dest: impl io::Write) -> anyhow::Result<()> {
        struct TableRow {
            bloat: Option<u32>,
            bloat_percent: Option<f64>,
            size: u32,
            size_percent: f64,
            name: String,
        }

        // Given an entry representing a generic function and its various
        // monomorphizations, return a vector of table rows.
        fn process_entry<'a>(
            entry: &'a MonosEntry,
            total_size: f64,
        ) -> impl Iterator<Item = TableRow> + 'a {
            let MonosEntry {
                name,
                insts,
                size,
                bloat,
            } = entry;

            let get_size_percent = move |x: u32| f64::from(x) / total_size * 100.0;

            iter::once(TableRow {
                bloat: Some(*bloat),
                bloat_percent: Some(get_size_percent(*bloat)),
                size: *size,
                size_percent: get_size_percent(*size),
                name: name.to_string(),
            })
            .chain(insts.iter().map(move |(name, size)| TableRow {
                bloat: None,
                bloat_percent: None,
                size: *size,
                size_percent: get_size_percent(*size),
                name: format!("    {}", name),
            }))
        }

        let mut table = Table::with_header(vec![
            (Align::Right, "Apprx. Bloat Bytes".into()),
            (Align::Right, "Apprx. Bloat %".into()),
            (Align::Right, "Bytes".into()),
            (Align::Right, "%".into()),
            (Align::Left, "Monomorphizations".to_string()),
        ]);

        for TableRow {
            bloat,
            bloat_percent,
            size,
            size_percent,
            name,
        } in self
            .monos
            .iter()
            .flat_map(|mono| process_entry(mono, f64::from(self.items.size())))
        {
            table.add_row(vec![
                bloat.map(|b| b.to_string()).unwrap_or_default(),
                bloat_percent
                    .map(|b| format!("{:.2}%", b))
                    .unwrap_or_default(),
                size.to_string(),
                format!("{:.2}%", size_percent),
                name.clone(),
            ]);
        }
        write!(dest, "{}", &table)?;
        Ok(())
    }

    #[cfg(feature = "emit_json")]
    fn emit_json(&self, dest: impl io::Write) -> anyhow::Result<()> {
        let items_size = f64::from(self.items.size());

        serde_json::to_writer_pretty(
            dest,
            &self
                .monos
                .iter()
                .map(|entry| {
                    let get_size_percent = |size| f64::from(size) / items_size * 100.0;

                    let monomorphizations = entry
                        .insts
                        .iter()
                        .map(|(name, size)| {
                            serde_json::json!({
                                "name": name,
                                "shallow_size": size,
                                "shallow_size_percent": get_size_percent(*size),
                            })
                        })
                        .collect::<Vec<_>>();

                    serde_json::json!({
                        "generic": entry.name,
                        "approximate_monomorphization_bloat_bytes": entry.bloat,
                        "approximate_monomorphization_bloat_percent": get_size_percent(entry.bloat),
                        "total_size": entry.size,
                        "total_size_percent": get_size_percent(entry.size),
                        "monomorphizations": monomorphizations,
                    })
                })
                .collect::<Vec<_>>(),
        )?;
        Ok(())
    }

    #[cfg(feature = "emit_csv")]
    fn emit_csv(&self, dest: impl io::Write) -> anyhow::Result<()> {
        // Calculate the total size of the collection of items, and define a
        // helper closure to calculate a percent value for a given u32 size.
        let items_size = f64::from(self.items.size());
        let get_size_percent = |size: u32| (f64::from(size)) / items_size * 100.0;

        #[derive(Debug, Default, serde::Serialize)]
        #[serde(rename_all = "PascalCase")]
        struct Record {
            generic: Option<String>,
            approximate_monomorphization_bloat_bytes: Option<u32>,
            approximate_monomorphization_bloat_percent: Option<f64>,
            total_size: Option<u32>,
            total_size_percent: Option<f64>,
            monomorphizations: Option<String>,
        }

        // Given a single `MonosEntry` object, create a record object.
        let process_entry = |entry: &MonosEntry| -> Record {
            let monos = entry
                .insts
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>();
            Record {
                generic: Some(entry.name.clone()),
                approximate_monomorphization_bloat_bytes: Some(entry.bloat),
                approximate_monomorphization_bloat_percent: Some(get_size_percent(entry.bloat)),
                total_size: Some(entry.size),
                total_size_percent: Some(get_size_percent(entry.size)),
                monomorphizations: Some(monos.join(", ")),
            }
        };

        // Create a CSV writer and iterate through the monomorphization entries.
        // Process each record and pass it to the destination to be serialized.
        let mut wtr = csv::Writer::from_writer(dest);
        for entry in &self.monos {
            let record = process_entry(entry);
            wtr.serialize(record)?;
            wtr.flush()?;
        }
        Ok(())
    }
}

/// Type alias used to represent a map of generic function names and instantiations.
type MonosMap<'a> = BTreeMap<&'a str, Vec<(String, u32)>>;

/// Collect the monomorphizations of generic functions into a map, then
/// process the entries and sort the resulting vector.
fn collect_monomorphizations<'a>(
    items: &'a ir::Items,
    opts: &Options,
) -> anyhow::Result<MonosMap<'a>> {
    let args_given = !opts.functions.is_empty();
    let using_regexps = opts.using_regexps;
    let regexps = regex::RegexSet::new(&opts.functions)?;

    let unsorted_monos: BTreeMap<&'a str, BTreeSet<(String, u32)>> = items
        .iter()
        .filter_map(|item| {
            if let Some(generic) = item.monomorphization_of() {
                Some((generic, item))
            } else {
                None
            }
        })
        .filter(|(generic, _)| match (args_given, using_regexps) {
            (true, true) => regexps.is_match(generic),
            (true, false) => opts.functions.iter().any(|name| name == generic),
            (false, _) => true,
        })
        .fold(BTreeMap::new(), |mut monos, (generic, inst)| {
            monos
                .entry(generic)
                .or_insert_with(BTreeSet::new)
                .insert((inst.name().to_string(), inst.size()));
            monos
        });

    Ok(unsorted_monos
        .into_iter()
        .map(|(generic, inst_set)| {
            let mut insts = inst_set.into_iter().collect::<Vec<_>>();
            insts.sort_by(|(a_name, a_size), (b_name, b_size)| {
                b_size.cmp(a_size).then(a_name.cmp(b_name))
            });
            (generic, insts)
        })
        .collect())
}

/// Helper function usedd to summarize a sequence of `MonosEntry` objects.
/// Returns a tuple representing the number of items summarized, the total
/// size of the items, and the total approximate potential savings.
fn summarize_entries<'a>(entries: impl Iterator<Item = &'a MonosEntry>) -> (usize, u32, u32) {
    entries.fold(
        (0, 0, 0),
        |(total_cnt, total_size, total_savings),
         MonosEntry {
             insts, size, bloat, ..
         }| {
            (
                total_cnt + 1 + insts.len(),
                total_size + size,
                total_savings + bloat,
            )
        },
    )
}

/// Helper function used to summarize a sequence of tuples representing
/// instantiations of a generic function. Returns a tuple representing the
/// number of instantiations found, and the total size.
fn summarize_insts<'a>(entries: impl Iterator<Item = &'a (String, u32)>) -> (u32, u32) {
    entries.fold((0, 0), |(total_cnt, total_size), (_, size)| {
        (total_cnt + 1, total_size + size)
    })
}

/// Find the approximate potential savings by calculating the benefits of
/// removing the largest instantiation, and the benefits of removing an
/// average instantiation. Returns a tuple containing total size, and bloat.
fn calculate_total_and_bloat(insts: &[(String, u32)]) -> Option<(u32, u32)> {
    if let Some(max) = insts.iter().map(|(_, size)| size).max() {
        let total_size = insts.iter().map(|(_, size)| size).sum::<u32>();
        let inst_cnt = insts.len() as u32;
        let size_per_inst = total_size / inst_cnt;
        let avg_savings = size_per_inst * (inst_cnt - 1);
        let removing_largest_savings = total_size - max;
        let approx_potential_savings = cmp::min(avg_savings, removing_largest_savings);
        Some((total_size, approx_potential_savings))
    } else {
        None
    }
}

/// Process all of the monorphizations, into a vector of `MonosEntry` objects.
fn process_monomorphizations(monos_map: MonosMap, opts: &Options) -> Vec<MonosEntry> {
    let mut monos = monos_map
        .into_iter()
        .filter_map(|(g, insts)| {
            calculate_total_and_bloat(&insts).map(|(total, bloat)| (g, insts, total, bloat))
        })
        .map(|(g, mut insts, t, b)| {
            // Truncate `insts` according to the relevant options before
            // we map these values into `MonosEntry` objects.
            if opts.only_generics {
                insts.truncate(0);
            } else {
                let max_monos = opts.max_monos as usize;
                let (rem_cnt, rem_size) = summarize_insts(insts.iter().skip(max_monos));
                insts.truncate(max_monos);
                if rem_cnt > 0 {
                    insts.push((format!("... and {} more.", rem_cnt), rem_size));
                }
            };
            (g, insts, t, b)
        })
        .map(|(name, insts, size, bloat)| MonosEntry {
            name: name.to_string(),
            insts,
            size,
            bloat,
        })
        .collect::<Vec<_>>();
    monos.sort();
    monos
}

/// Adds entries to summarize remaining rows that will be truncated, and
/// totals for the entire set of monomorphizations.
fn add_stats(mut monos: Vec<MonosEntry>, opts: Options) -> Vec<MonosEntry> {
    let max_generics = opts.max_generics as usize;

    // Create an entry to represent the remaining rows that will be truncated,
    // only if there are more generics than we will display.
    let remaining: Option<MonosEntry> = {
        if monos.len() > max_generics {
            let (rem_cnt, rem_size, rem_savings) =
                summarize_entries(monos.iter().skip(max_generics));
            Some(MonosEntry {
                name: format!("... and {} more.", rem_cnt),
                size: rem_size,
                insts: vec![],
                bloat: rem_savings,
            })
        } else {
            None
        }
    };

    // Create an entry to represent the 'total' summary.
    let total = {
        let (total_cnt, total_size, total_savings) = summarize_entries(monos.iter());
        MonosEntry {
            name: format!("Σ [{} Total Rows]", total_cnt),
            size: total_size,
            insts: vec![],
            bloat: total_savings,
        }
    };

    // Truncate the vector, and add the 'remaining' and 'total' summary entries.
    monos.truncate(max_generics);
    if let Some(remaining) = remaining {
        monos.push(remaining);
    }
    monos.push(total);
    monos
}

/// Find bloaty monomorphizations of generic functions.
pub fn monos(items: &mut ir::Items, opts: Options) -> anyhow::Result<Report<'_>> {
    let monos_map = collect_monomorphizations(items, &opts)?;
    let mut monos = process_monomorphizations(monos_map, &opts);
    monos = add_stats(monos, opts);
    Ok(Report { monos, items })
}
