//! Deterministic Core-only 3MF serialization and OPC package creation.

mod model;
mod package;

use std::{error::Error, fmt, io};

use plaque3mf_mesh25d::PartModel;

/// Largest resource count representable by a Core 3MF resource ID.
pub const MAX_3MF_OBJECTS: usize = 2_147_483_647;

/// Largest entry or archive size emitted by the plain ZIP32 package writer.
pub const MAX_ZIP32_BYTES: u64 = u32::MAX as u64;

/// Writes a deterministic Core-only 3MF package to an empty destination.
///
/// The package contains named substrate and fill mesh resources, one parent
/// assembly that preserves their alignment, and one build item. Coordinates
/// are converted exactly from integer micrometres to decimal millimetres.
/// The first byte written is the start of the ZIP archive; callers must not
/// prepend data to the destination.
///
/// Output may be partial if the writer returns an error; callers publishing to
/// a filesystem should therefore write to a temporary file and rename it only
/// after this function succeeds.
pub fn write_3mf<W: io::Write>(model: &PartModel, writer: &mut W) -> Result<(), ExportError> {
    let object_count = model
        .fill_parts()
        .len()
        .checked_add(2)
        .ok_or(ExportError::ArithmeticOverflow)?;
    if object_count > MAX_3MF_OBJECTS {
        return Err(ExportError::TooManyObjects {
            required: object_count,
            max: MAX_3MF_OBJECTS,
        });
    }
    let assembly_id = u32::try_from(object_count).map_err(|_| ExportError::TooManyObjects {
        required: object_count,
        max: MAX_3MF_OBJECTS,
    })?;
    package::write_package(model, assembly_id, writer)
}

/// A failure while serializing a validated part model as 3MF.
#[derive(Debug)]
#[non_exhaustive]
pub enum ExportError {
    /// The model cannot be assigned valid positive Core resource IDs.
    TooManyObjects { required: usize, max: usize },
    /// One package part cannot be represented by a plain ZIP32 entry.
    EntryTooLarge {
        part: &'static str,
        required: u64,
        max: u64,
    },
    /// The complete archive cannot be represented without ZIP64.
    PackageTooLarge { required: u64, max: u64 },
    /// Checked package-size arithmetic overflowed.
    ArithmeticOverflow,
    /// The repeated streaming pass produced different model XML.
    SerializationMismatch { part: &'static str },
    /// The destination writer failed.
    Io(io::Error),
}

impl fmt::Display for ExportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyObjects { required, max } => {
                write!(
                    formatter,
                    "3MF model requires {required} objects; the limit is {max}"
                )
            }
            Self::EntryTooLarge {
                part,
                required,
                max,
            } => write!(
                formatter,
                "3MF package part {part} requires {required} bytes; the ZIP32 limit is {max}"
            ),
            Self::PackageTooLarge { required, max } => write!(
                formatter,
                "3MF package requires {required} bytes; the ZIP32 limit is {max}"
            ),
            Self::ArithmeticOverflow => {
                formatter.write_str("3MF package size arithmetic overflowed")
            }
            Self::SerializationMismatch { part } => {
                write!(
                    formatter,
                    "3MF package part {part} changed between streaming passes"
                )
            }
            Self::Io(error) => write!(formatter, "could not write 3MF package: {error}"),
        }
    }
}

impl Error for ExportError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for ExportError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(test)]
mod tests;
