// SPDX-License-Identifier: GPL-3.0-or-later
//! The project model and the `.las` file format of Lumen Audio Studio.
//!
//! Projects saved by the web version open unchanged: [`las::decode`] reads the
//! file, [`migrate::migrate_project`] brings old projects up to date and repairs
//! damaged data, and [`las::encode`] writes a file the web version can read.

pub mod create;
pub mod las;
pub mod migrate;
pub mod model;
pub mod slicer;

pub use las::{decode, encode, Bundle, LasError, Sample};
pub use migrate::{migrate_project, Migrated, ProjectError};
pub use model::*;
