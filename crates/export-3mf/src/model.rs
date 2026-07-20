//! Deterministic serialization of the Core 3MF model part.

use std::io::{self, Write};

use plaque3mf_mesh25d::{PartModel, TriangleMesh};

const XML_DECLARATION: &[u8] = b"<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";
const MODEL_START: &[u8] = b"<model unit=\"millimeter\" xml:lang=\"en-US\" xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\">\n  <resources>\n";

/// Writes the package's single Core 3MF model part.
pub(crate) fn write_model_xml<W: Write>(
    model: &PartModel,
    assembly_id: u32,
    writer: &mut W,
) -> io::Result<()> {
    writer.write_all(XML_DECLARATION)?;
    writer.write_all(MODEL_START)?;

    writer.write_all(b"    <object id=\"1\" type=\"model\" name=\"substrate\">\n")?;
    write_mesh_xml(model.substrate(), writer)?;
    writer.write_all(b"    </object>\n")?;

    for (index, mesh) in model.fill_parts().iter().enumerate() {
        let object_id = u32::try_from(index + 2)
            .expect("mesh construction limits keep fill object IDs within u32");
        let fill_number = index + 1;
        writeln!(
            writer,
            "    <object id=\"{object_id}\" type=\"model\" name=\"fill-{fill_number:04}\">"
        )?;
        write_mesh_xml(mesh, writer)?;
        writer.write_all(b"    </object>\n")?;
    }

    writeln!(
        writer,
        "    <object id=\"{assembly_id}\" type=\"model\" name=\"plaque3mf-assembly\">"
    )?;
    writer.write_all(b"      <components>\n")?;
    writer.write_all(b"        <component objectid=\"1\" />\n")?;
    for index in 0..model.fill_parts().len() {
        let object_id = u32::try_from(index + 2)
            .expect("mesh construction limits keep fill object IDs within u32");
        writeln!(writer, "        <component objectid=\"{object_id}\" />")?;
    }
    writer.write_all(b"      </components>\n")?;
    writer.write_all(b"    </object>\n")?;
    writer.write_all(b"  </resources>\n")?;
    writer.write_all(b"  <build>\n")?;
    writeln!(writer, "    <item objectid=\"{assembly_id}\" />")?;
    writer.write_all(b"  </build>\n")?;
    writer.write_all(b"</model>\n")
}

fn write_mesh_xml<W: Write>(mesh: &TriangleMesh, writer: &mut W) -> io::Result<()> {
    writer.write_all(b"      <mesh>\n")?;
    writer.write_all(b"        <vertices>\n")?;
    for vertex in mesh.vertices() {
        writer.write_all(b"          <vertex x=\"")?;
        write_millimeters(writer, vertex.x())?;
        writer.write_all(b"\" y=\"")?;
        write_millimeters(writer, vertex.y())?;
        writer.write_all(b"\" z=\"")?;
        write_millimeters(writer, vertex.z())?;
        writer.write_all(b"\" />\n")?;
    }
    writer.write_all(b"        </vertices>\n")?;
    writer.write_all(b"        <triangles>\n")?;
    for triangle in mesh.triangles() {
        let [v1, v2, v3] = triangle.indices();
        writeln!(
            writer,
            "          <triangle v1=\"{v1}\" v2=\"{v2}\" v3=\"{v3}\" />"
        )?;
    }
    writer.write_all(b"        </triangles>\n")?;
    writer.write_all(b"      </mesh>\n")
}

/// Writes an integer micrometre coordinate as an exact millimetre decimal.
fn write_millimeters<W: Write>(writer: &mut W, micrometers: i64) -> io::Result<()> {
    let value = i128::from(micrometers);
    if value < 0 {
        writer.write_all(b"-")?;
    }

    let magnitude = value.abs();
    let whole = magnitude / 1_000;
    let fractional = magnitude % 1_000;
    write!(writer, "{whole}")?;

    if fractional == 0 {
        return Ok(());
    }
    if fractional % 100 == 0 {
        return write!(writer, ".{}", fractional / 100);
    }
    if fractional % 10 == 0 {
        return write!(writer, ".{:02}", fractional / 10);
    }
    write!(writer, ".{fractional:03}")
}

#[cfg(test)]
mod tests {
    use super::write_millimeters;

    fn formatted(micrometers: i64) -> String {
        let mut bytes = Vec::new();
        write_millimeters(&mut bytes, micrometers).expect("coordinate formats");
        String::from_utf8(bytes).expect("formatter emits ASCII")
    }

    #[test]
    fn whole_millimeters_have_no_fractional_suffix() {
        assert_eq!(formatted(0), "0");
        assert_eq!(formatted(1_000), "1");
        assert_eq!(formatted(12_345_000), "12345");
        assert_eq!(formatted(-12_345_000), "-12345");
    }

    #[test]
    fn fractional_millimeters_are_exact_and_minimal() {
        for (micrometers, expected) in [
            (1, "0.001"),
            (10, "0.01"),
            (100, "0.1"),
            (101, "0.101"),
            (110, "0.11"),
            (1_010, "1.01"),
            (-1, "-0.001"),
            (-1_010, "-1.01"),
        ] {
            assert_eq!(formatted(micrometers), expected);
        }
    }

    #[test]
    fn full_i64_range_formats_without_float_rounding() {
        assert_eq!(formatted(i64::MAX), "9223372036854775.807");
        assert_eq!(formatted(i64::MIN), "-9223372036854775.808");
    }
}
