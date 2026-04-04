//! Implementations of the analyses that `twiggy` runs on its IR.

mod analyses;
mod formats;

pub use analyses::*;

use derive_more::{Display, FromStr};

pub use diff::diff;
pub use dominators::dominators;
pub use garbage::garbage;
pub use monos::monos;
pub use paths::paths;
pub use top::top;

#[derive(Display, FromStr, Default, Clone, Copy, Debug)]
#[display(rename_all = "lowercase")]
pub enum OutputFormat {
    #[default]
    Text,
    #[cfg(feature = "emit_json")]
    Json,
    #[cfg(feature = "emit_csv")]
    Csv,
}
