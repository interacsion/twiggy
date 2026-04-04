//! The `twiggy` code size profiler.

use std::{io, path::PathBuf};

use clap::Parser;
use twiggy_analyze as analyze;
use twiggy_parser::{self as parser, ParseMode};

#[derive(Parser)]
/// Twiggy is a code size profiler.
///
/// It analyzes a binary's call graph to answer questions like:
///
/// * Why was this function included in the binary in the first place?
///
/// * What is the retained size of this function? I.e. how much space would be saved if I removed it and all the functions that become dead code after its removal.
///
/// Use twiggy to make your binaries slim!
enum Command {
    /// List the top code size offenders in a binary.
    Top {
        /// The path to the input binary to size profile.
        input: PathBuf,

        /// The parse mode for the input binary data.
        #[arg(long = "mode", default_value_t)]
        parse_mode: ParseMode,

        #[command(flatten)]
        options: analyze::top::Options,
        #[command(flatten)]
        emit_options: analyze::top::EmitOptions,
    },

    /// Compute and display the dominator tree for a binary's call graph.
    Dominators {
        /// The path to the input binary to size profile.
        input: PathBuf,

        /// The parse mode for the input binary data.
        #[arg(long = "mode", default_value_t)]
        parse_mode: ParseMode,

        #[command(flatten)]
        options: analyze::dominators::Options,
        #[command(flatten)]
        emit_options: analyze::dominators::EmitOptions,
    },

    /// Find and display the call paths to a function in the given binary's call
    /// graph.
    Paths {
        /// The path to the input binary to size profile.
        input: PathBuf,

        /// The parse mode for the input binary data.
        #[arg(long = "mode", default_value_t)]
        parse_mode: ParseMode,

        #[command(flatten)]
        options: analyze::paths::Options,
        #[command(flatten)]
        emit_options: analyze::paths::EmitOptions,
    },

    /// List the generic function monomorphizations that are contributing to
    /// code bloat.
    Monos {
        /// The path to the input binary to size profile.
        input: PathBuf,

        /// The parse mode for the input binary data.
        #[arg(long = "mode", default_value_t)]
        parse_mode: ParseMode,

        #[command(flatten)]
        options: analyze::monos::Options,
        #[command(flatten)]
        emit_options: analyze::monos::EmitOptions,
    },

    /// Diff the old and new versions of a binary to see what sizes changed.
    Diff {
        /// The path to the old version of the binary.
        old_input: PathBuf,

        /// The path to the new version of the binary.
        new_input: PathBuf,

        /// The parse mode for the input binary data.
        #[arg(long = "mode", default_value_t)]
        parse_mode: ParseMode,

        #[command(flatten)]
        options: analyze::diff::Options,
        #[command(flatten)]
        emit_options: analyze::diff::EmitOptions,
    },

    /// Find and display code and data that is not transitively referenced by
    /// any exports or public functions.
    Garbage {
        /// The path to the input binary to size profile.
        input: PathBuf,

        /// The parse mode for the input binary data.
        #[arg(long = "mode", default_value_t)]
        parse_mode: ParseMode,

        #[command(flatten)]
        options: analyze::garbage::Options,
        #[command(flatten)]
        emit_options: analyze::garbage::EmitOptions,
    },
}

fn main() -> anyhow::Result<()> {
    let command = Command::parse();

    match command {
        Command::Top {
            input,
            parse_mode,
            options,
            emit_options,
        } => {
            let mut items = parser::read_and_parse(input, parse_mode)?;
            analyze::top(&mut items, options)?.emit(emit_options, io::stdout())?;
        }
        Command::Dominators {
            input,
            parse_mode,
            options,
            emit_options,
        } => {
            let mut items = parser::read_and_parse(input, parse_mode)?;
            analyze::dominators(&mut items, options)?.emit(emit_options, io::stdout())?;
        }
        Command::Paths {
            input,
            parse_mode,
            options,
            emit_options,
        } => {
            let mut items = parser::read_and_parse(input, parse_mode)?;
            analyze::paths(&mut items, options)?.emit(emit_options, io::stdout())?;
        }
        Command::Monos {
            input,
            parse_mode,
            options,
            emit_options,
        } => {
            let mut items = parser::read_and_parse(input, parse_mode)?;
            analyze::monos(&mut items, options)?.emit(emit_options, io::stdout())?;
        }
        Command::Diff {
            old_input,
            new_input,
            parse_mode,
            options,
            emit_options,
        } =>  {
            let mut old_items = parser::read_and_parse(old_input, parse_mode)?;
            let mut new_items = parser::read_and_parse(new_input, parse_mode)?;
            analyze::diff(&mut old_items, &mut new_items, options)?.emit(emit_options, io::stdout())?;
        }
        Command::Garbage {
            input,
            parse_mode,
            options,
            emit_options,
        } => {
            let mut items = parser::read_and_parse(input, parse_mode)?;
            analyze::garbage(&mut items, options)?.emit(emit_options, io::stdout())?;
        }
    };

    Ok(())
}
