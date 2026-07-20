//! Minimal deterministic ZIP32 packaging for the three required OPC parts.

use std::io::{self, BufWriter, Write};

use plaque3mf_mesh25d::PartModel;

use crate::{ExportError, MAX_ZIP32_BYTES, model::write_model_xml};

pub(crate) const CONTENT_TYPES_PATH: &str = "[Content_Types].xml";
pub(crate) const RELATIONSHIPS_PATH: &str = "_rels/.rels";
pub(crate) const MODEL_PATH: &str = "3D/3dmodel.model";

pub(crate) const CONTENT_TYPES_XML: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
</Types>
"#;

pub(crate) const RELATIONSHIPS_XML: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
</Relationships>
"#;

const LOCAL_FILE_HEADER_SIGNATURE: u32 = 0x0403_4b50;
const CENTRAL_DIRECTORY_HEADER_SIGNATURE: u32 = 0x0201_4b50;
const END_OF_CENTRAL_DIRECTORY_SIGNATURE: u32 = 0x0605_4b50;
const STORED_METHOD: u16 = 0;
const ZIP_VERSION_2_0: u16 = 20;
const DOS_TIME_MIDNIGHT: u16 = 0;
const DOS_DATE_1980_01_01: u16 = 0x0021;
const LOCAL_HEADER_BYTES: u64 = 30;
const CENTRAL_HEADER_BYTES: u64 = 46;
const END_RECORD_BYTES: u64 = 22;
const ENTRY_COUNT: u16 = 3;
const BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Measurement {
    bytes: u64,
    crc32: u32,
}

#[derive(Debug, Clone, Copy)]
struct EntryPlan {
    name: &'static str,
    name_length: u16,
    size: u32,
    crc32: u32,
    local_header_offset: u32,
}

#[derive(Debug, Clone, Copy)]
struct ArchivePlan {
    entries: [EntryPlan; ENTRY_COUNT as usize],
    central_directory_offset: u32,
    central_directory_size: u32,
}

pub(crate) fn write_package<W: Write>(
    model: &PartModel,
    assembly_id: u32,
    writer: &mut W,
) -> Result<(), ExportError> {
    let model_measurement = measure_model(model, assembly_id)?;
    let plan = plan_archive(model_measurement)?;
    let mut writer = BufWriter::with_capacity(BUFFER_BYTES, writer);

    write_local_header(&mut writer, plan.entries[0])?;
    writer.write_all(CONTENT_TYPES_XML)?;
    write_local_header(&mut writer, plan.entries[1])?;
    writer.write_all(RELATIONSHIPS_XML)?;
    write_local_header(&mut writer, plan.entries[2])?;
    let actual_model = {
        let mut verifier = VerifyingWriter::new(&mut writer);
        write_model_xml(model, assembly_id, &mut verifier)?;
        verifier.finish()?
    };
    if actual_model != model_measurement {
        return Err(ExportError::SerializationMismatch { part: MODEL_PATH });
    }

    for entry in plan.entries {
        write_central_directory_header(&mut writer, entry)?;
    }
    write_end_record(&mut writer, plan)?;
    writer
        .into_inner()
        .map_err(|error| ExportError::Io(error.into_error()))?;
    Ok(())
}

fn measure_model(model: &PartModel, assembly_id: u32) -> Result<Measurement, ExportError> {
    let mut writer = MeasurementWriter::new();
    write_model_xml(model, assembly_id, &mut writer)
        .map_err(|_| ExportError::ArithmeticOverflow)?;
    writer.finish()
}

fn measurement_for(bytes: &[u8]) -> Result<Measurement, ExportError> {
    let byte_count = u64::try_from(bytes.len()).map_err(|_| ExportError::ArithmeticOverflow)?;
    let mut crc = Crc32::new();
    crc.update(bytes);
    Ok(Measurement {
        bytes: byte_count,
        crc32: crc.finish(),
    })
}

fn plan_archive(model: Measurement) -> Result<ArchivePlan, ExportError> {
    let mut entries = [
        entry_plan(CONTENT_TYPES_PATH, measurement_for(CONTENT_TYPES_XML)?)?,
        entry_plan(RELATIONSHIPS_PATH, measurement_for(RELATIONSHIPS_XML)?)?,
        entry_plan(MODEL_PATH, model)?,
    ];
    let mut cursor = 0_u64;
    for entry in &mut entries {
        entry.local_header_offset = package_u32(cursor)?;
        cursor = cursor
            .checked_add(LOCAL_HEADER_BYTES)
            .and_then(|value| value.checked_add(u64::from(entry.name_length)))
            .and_then(|value| value.checked_add(u64::from(entry.size)))
            .ok_or(ExportError::ArithmeticOverflow)?;
    }
    let central_directory_offset = package_u32(cursor)?;
    let central_start = cursor;
    for entry in &entries {
        cursor = cursor
            .checked_add(CENTRAL_HEADER_BYTES)
            .and_then(|value| value.checked_add(u64::from(entry.name_length)))
            .ok_or(ExportError::ArithmeticOverflow)?;
    }
    let central_directory_size = package_u32(
        cursor
            .checked_sub(central_start)
            .ok_or(ExportError::ArithmeticOverflow)?,
    )?;
    let archive_size = cursor
        .checked_add(END_RECORD_BYTES)
        .ok_or(ExportError::ArithmeticOverflow)?;
    if archive_size > MAX_ZIP32_BYTES {
        return Err(ExportError::PackageTooLarge {
            required: archive_size,
            max: MAX_ZIP32_BYTES,
        });
    }
    Ok(ArchivePlan {
        entries,
        central_directory_offset,
        central_directory_size,
    })
}

fn entry_plan(name: &'static str, measurement: Measurement) -> Result<EntryPlan, ExportError> {
    let name_length = u16::try_from(name.len()).map_err(|_| ExportError::ArithmeticOverflow)?;
    let size = u32::try_from(measurement.bytes).map_err(|_| ExportError::EntryTooLarge {
        part: name,
        required: measurement.bytes,
        max: MAX_ZIP32_BYTES,
    })?;
    Ok(EntryPlan {
        name,
        name_length,
        size,
        crc32: measurement.crc32,
        local_header_offset: 0,
    })
}

fn package_u32(required: u64) -> Result<u32, ExportError> {
    u32::try_from(required).map_err(|_| ExportError::PackageTooLarge {
        required,
        max: MAX_ZIP32_BYTES,
    })
}

fn write_local_header<W: Write>(writer: &mut W, entry: EntryPlan) -> io::Result<()> {
    write_u32(writer, LOCAL_FILE_HEADER_SIGNATURE)?;
    write_u16(writer, ZIP_VERSION_2_0)?;
    write_u16(writer, 0)?;
    write_u16(writer, STORED_METHOD)?;
    write_u16(writer, DOS_TIME_MIDNIGHT)?;
    write_u16(writer, DOS_DATE_1980_01_01)?;
    write_u32(writer, entry.crc32)?;
    write_u32(writer, entry.size)?;
    write_u32(writer, entry.size)?;
    write_u16(writer, entry.name_length)?;
    write_u16(writer, 0)?;
    writer.write_all(entry.name.as_bytes())
}

fn write_central_directory_header<W: Write>(writer: &mut W, entry: EntryPlan) -> io::Result<()> {
    write_u32(writer, CENTRAL_DIRECTORY_HEADER_SIGNATURE)?;
    write_u16(writer, ZIP_VERSION_2_0)?;
    write_u16(writer, ZIP_VERSION_2_0)?;
    write_u16(writer, 0)?;
    write_u16(writer, STORED_METHOD)?;
    write_u16(writer, DOS_TIME_MIDNIGHT)?;
    write_u16(writer, DOS_DATE_1980_01_01)?;
    write_u32(writer, entry.crc32)?;
    write_u32(writer, entry.size)?;
    write_u32(writer, entry.size)?;
    write_u16(writer, entry.name_length)?;
    write_u16(writer, 0)?;
    write_u16(writer, 0)?;
    write_u16(writer, 0)?;
    write_u16(writer, 0)?;
    write_u32(writer, 0)?;
    write_u32(writer, entry.local_header_offset)?;
    writer.write_all(entry.name.as_bytes())
}

fn write_end_record<W: Write>(writer: &mut W, plan: ArchivePlan) -> io::Result<()> {
    write_u32(writer, END_OF_CENTRAL_DIRECTORY_SIGNATURE)?;
    write_u16(writer, 0)?;
    write_u16(writer, 0)?;
    write_u16(writer, ENTRY_COUNT)?;
    write_u16(writer, ENTRY_COUNT)?;
    write_u32(writer, plan.central_directory_size)?;
    write_u32(writer, plan.central_directory_offset)?;
    write_u16(writer, 0)
}

fn write_u16<W: Write>(writer: &mut W, value: u16) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

fn write_u32<W: Write>(writer: &mut W, value: u32) -> io::Result<()> {
    writer.write_all(&value.to_le_bytes())
}

struct MeasurementWriter {
    bytes: u64,
    crc32: Crc32,
    overflowed: bool,
}

impl MeasurementWriter {
    const fn new() -> Self {
        Self {
            bytes: 0,
            crc32: Crc32::new(),
            overflowed: false,
        }
    }

    fn finish(self) -> Result<Measurement, ExportError> {
        if self.overflowed {
            return Err(ExportError::ArithmeticOverflow);
        }
        Ok(Measurement {
            bytes: self.bytes,
            crc32: self.crc32.finish(),
        })
    }

    fn record(&mut self, buffer: &[u8]) {
        let byte_count = u64::try_from(buffer.len()).unwrap_or(u64::MAX);
        if let Some(total) = self.bytes.checked_add(byte_count) {
            self.bytes = total;
        } else {
            self.bytes = u64::MAX;
            self.overflowed = true;
        }
        self.crc32.update(buffer);
    }
}

impl Write for MeasurementWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.record(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct VerifyingWriter<W> {
    inner: W,
    measurement: MeasurementWriter,
}

impl<W: Write> VerifyingWriter<W> {
    const fn new(inner: W) -> Self {
        Self {
            inner,
            measurement: MeasurementWriter::new(),
        }
    }

    fn finish(self) -> Result<Measurement, ExportError> {
        self.measurement.finish()
    }
}

impl<W: Write> Write for VerifyingWriter<W> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buffer)?;
        self.measurement.record(&buffer[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

#[derive(Debug, Clone, Copy)]
struct Crc32(u32);

impl Crc32 {
    const fn new() -> Self {
        Self(u32::MAX)
    }

    fn update(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            let index = ((self.0 ^ u32::from(byte)) & 0xff) as usize;
            self.0 = (self.0 >> 8) ^ CRC32_TABLE[index];
        }
    }

    const fn finish(self) -> u32 {
        !self.0
    }
}

const fn make_crc32_table() -> [u32; 256] {
    let mut table = [0_u32; 256];
    let mut index = 0_usize;
    while index < table.len() {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 1 == 1 {
                (value >> 1) ^ 0xedb8_8320
            } else {
                value >> 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

const CRC32_TABLE: [u32; 256] = make_crc32_table();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_standard_check_vector() {
        let mut crc = Crc32::new();
        crc.update(b"123456789");
        assert_eq!(crc.finish(), 0xcbf4_3926);
    }

    #[test]
    fn zip32_bounds_are_typed_at_the_exact_boundary() {
        assert_eq!(
            package_u32(MAX_ZIP32_BYTES).expect("the ZIP32 boundary fits"),
            u32::MAX
        );
        assert!(matches!(
            package_u32(MAX_ZIP32_BYTES + 1),
            Err(ExportError::PackageTooLarge {
                required,
                max: MAX_ZIP32_BYTES
            }) if required == MAX_ZIP32_BYTES + 1
        ));
        assert!(matches!(
            entry_plan(
                MODEL_PATH,
                Measurement {
                    bytes: MAX_ZIP32_BYTES + 1,
                    crc32: 0,
                },
            ),
            Err(ExportError::EntryTooLarge {
                part: MODEL_PATH,
                required,
                max: MAX_ZIP32_BYTES
            }) if required == MAX_ZIP32_BYTES + 1
        ));
    }
}
