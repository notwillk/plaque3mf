use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error as StdError,
    io::{self, Write},
    str,
};

use plaque3mf_document::{BinaryMask, CanonicalArtwork, PhysicalSizeMicrometers};
use plaque3mf_mesh25d::{MeshOptions, PartModel, TriangleMesh, build_part_model};
use plaque3mf_planar::{PlanarOptions, partition_artwork};

use super::write_3mf;

const BACKING_MICROMETERS: i64 = 101;
const TOTAL_MICROMETERS: i64 = 251;
const CONTENT_TYPES_PATH: &str = "[Content_Types].xml";
const RELATIONSHIPS_PATH: &str = "_rels/.rels";
const MODEL_PATH: &str = "3D/3dmodel.model";
const EXPECTED_PATHS: [&str; 3] = [CONTENT_TYPES_PATH, RELATIONSHIPS_PATH, MODEL_PATH];

const LOCAL_FILE_HEADER_SIGNATURE: u32 = 0x0403_4b50;
const CENTRAL_DIRECTORY_SIGNATURE: u32 = 0x0201_4b50;
const END_OF_CENTRAL_DIRECTORY_SIGNATURE: u32 = 0x0605_4b50;
const STORED_COMPRESSION_METHOD: u16 = 0;
const DOS_TIME_MIDNIGHT: u16 = 0;
const DOS_DATE_1980_01_01: u16 = 0x0021;

const CONTENT_TYPES_NAMESPACE: &str =
    "http://schemas.openxmlformats.org/package/2006/content-types";
const RELATIONSHIPS_NAMESPACE: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships";
const RELATIONSHIP_CONTENT_TYPE: &str = "application/vnd.openxmlformats-package.relationships+xml";
const MODEL_CONTENT_TYPE: &str = "application/vnd.ms-package.3dmanufacturing-3dmodel+xml";
const START_PART_RELATIONSHIP: &str =
    "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";
const CORE_MODEL_NAMESPACE: &str = "http://schemas.microsoft.com/3dmanufacturing/core/2015/02";

fn build_fixture(
    physical_width: i64,
    physical_height: i64,
    pixel_width: u32,
    pixel_height: u32,
    pixels: &[u8],
) -> PartModel {
    let artwork = CanonicalArtwork::new(
        PhysicalSizeMicrometers::new(physical_width, physical_height)
            .expect("fixture physical size is valid"),
        BinaryMask::new(pixel_width, pixel_height, pixels.to_vec()).expect("fixture mask is valid"),
    );
    let partition = partition_artwork(
        &artwork,
        PlanarOptions::new(0, 1, 0).expect("fixture planar options are valid"),
    )
    .expect("fixture partitions");
    build_part_model(
        &partition,
        MeshOptions::new(BACKING_MICROMETERS, TOTAL_MICROMETERS)
            .expect("fixture mesh options are valid"),
    )
    .expect("fixture meshes")
}

fn multi_fill_fixture() -> PartModel {
    let model = build_fixture(3_001, 1_001, 3, 1, &[0, 1, 0]);
    assert_eq!(model.fill_parts().len(), 2, "fixture must have two fills");
    model
}

fn substrate_only_fixture() -> PartModel {
    let model = build_fixture(1_001, 1_001, 1, 1, &[1]);
    assert!(model.fill_parts().is_empty());
    model
}

fn one_fill_fixture() -> PartModel {
    let model = build_fixture(1_001, 1_001, 1, 1, &[0]);
    assert_eq!(model.fill_parts().len(), 1);
    model
}

fn archive(model: &PartModel) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_3mf(model, &mut bytes).expect("fixture serializes");
    bytes
}

#[derive(Debug)]
struct CentralRecord {
    name: String,
    flags: u16,
    method: u16,
    modified_time: u16,
    modified_date: u16,
    crc32: u32,
    compressed_size: u32,
    uncompressed_size: u32,
    local_header_offset: u32,
}

#[derive(Debug)]
struct ZipEntry<'a> {
    name: String,
    data: &'a [u8],
}

fn parse_zip(bytes: &[u8]) -> Vec<ZipEntry<'_>> {
    assert!(bytes.len() >= 22, "archive is shorter than a ZIP EOCD");
    let eocd = bytes.len() - 22;
    assert_eq!(read_u32(bytes, eocd), END_OF_CENTRAL_DIRECTORY_SIGNATURE);
    assert_eq!(read_u16(bytes, eocd + 4), 0, "multi-disk ZIP is forbidden");
    assert_eq!(read_u16(bytes, eocd + 6), 0, "multi-disk ZIP is forbidden");
    let entries_on_disk = read_u16(bytes, eocd + 8);
    let entry_count = read_u16(bytes, eocd + 10);
    assert_eq!(entries_on_disk, entry_count);
    assert_eq!(usize::from(entry_count), EXPECTED_PATHS.len());
    let central_size = as_usize(read_u32(bytes, eocd + 12));
    let central_offset = as_usize(read_u32(bytes, eocd + 16));
    assert_eq!(read_u16(bytes, eocd + 20), 0, "ZIP comment must be empty");
    assert_eq!(central_offset + central_size, eocd);

    let mut cursor = central_offset;
    let mut records = Vec::with_capacity(usize::from(entry_count));
    let mut names = BTreeSet::new();
    for expected_name in EXPECTED_PATHS {
        assert_eq!(read_u32(bytes, cursor), CENTRAL_DIRECTORY_SIGNATURE);
        let flags = read_u16(bytes, cursor + 8);
        let method = read_u16(bytes, cursor + 10);
        let modified_time = read_u16(bytes, cursor + 12);
        let modified_date = read_u16(bytes, cursor + 14);
        let crc32 = read_u32(bytes, cursor + 16);
        let compressed_size = read_u32(bytes, cursor + 20);
        let uncompressed_size = read_u32(bytes, cursor + 24);
        let name_length = usize::from(read_u16(bytes, cursor + 28));
        let extra_length = usize::from(read_u16(bytes, cursor + 30));
        let comment_length = usize::from(read_u16(bytes, cursor + 32));
        assert_eq!(read_u16(bytes, cursor + 34), 0, "entry uses another disk");
        assert_eq!(read_u16(bytes, cursor + 36), 0, "internal attributes vary");
        assert_eq!(read_u32(bytes, cursor + 38), 0, "external attributes vary");
        let local_header_offset = read_u32(bytes, cursor + 42);
        let name_start = cursor + 46;
        let name_end = name_start + name_length;
        let name = str::from_utf8(slice(bytes, name_start, name_end))
            .expect("ZIP entry path is UTF-8")
            .to_owned();

        assert_eq!(name, expected_name);
        assert!(names.insert(name.clone()), "ZIP path is duplicated");
        assert!(!name.starts_with('/'));
        assert!(!name.ends_with('/'));
        assert!(!name.split('/').any(|segment| segment == ".."));
        assert_eq!(flags, 0, "unexpected ZIP general-purpose flags");
        assert_eq!(method, STORED_COMPRESSION_METHOD);
        assert_eq!(modified_time, DOS_TIME_MIDNIGHT);
        assert_eq!(modified_date, DOS_DATE_1980_01_01);
        assert_eq!(compressed_size, uncompressed_size);
        assert_eq!(extra_length, 0, "ZIP extra fields are forbidden");
        assert_eq!(comment_length, 0, "per-entry ZIP comments are forbidden");

        records.push(CentralRecord {
            name,
            flags,
            method,
            modified_time,
            modified_date,
            crc32,
            compressed_size,
            uncompressed_size,
            local_header_offset,
        });
        cursor = name_end + extra_length + comment_length;
    }
    assert_eq!(cursor, eocd);

    let mut entries = Vec::with_capacity(records.len());
    for (index, record) in records.iter().enumerate() {
        let local = as_usize(record.local_header_offset);
        assert_eq!(read_u32(bytes, local), LOCAL_FILE_HEADER_SIGNATURE);
        assert_eq!(read_u16(bytes, local + 6), record.flags);
        assert_eq!(read_u16(bytes, local + 8), record.method);
        assert_eq!(read_u16(bytes, local + 10), record.modified_time);
        assert_eq!(read_u16(bytes, local + 12), record.modified_date);
        assert_eq!(read_u32(bytes, local + 14), record.crc32);
        assert_eq!(read_u32(bytes, local + 18), record.compressed_size);
        assert_eq!(read_u32(bytes, local + 22), record.uncompressed_size);
        let name_length = usize::from(read_u16(bytes, local + 26));
        let extra_length = usize::from(read_u16(bytes, local + 28));
        assert_eq!(extra_length, 0, "local ZIP extra fields are forbidden");
        let name_start = local + 30;
        let name_end = name_start + name_length;
        assert_eq!(
            str::from_utf8(slice(bytes, name_start, name_end)).expect("local path is UTF-8"),
            record.name
        );
        let data_start = name_end + extra_length;
        let data_end = data_start + as_usize(record.compressed_size);
        let expected_next_offset = records
            .get(index + 1)
            .map_or(central_offset, |next| as_usize(next.local_header_offset));
        assert_eq!(data_end, expected_next_offset, "ZIP records contain a gap");
        let data = slice(bytes, data_start, data_end);
        assert_eq!(as_usize(record.uncompressed_size), data.len());
        assert_eq!(record.crc32, crc32(data));
        entries.push(ZipEntry {
            name: record.name.clone(),
            data,
        });
    }
    assert_eq!(
        records.first().map(|record| record.local_header_offset),
        Some(0),
        "first local header must begin the archive"
    );
    entries
}

fn entry<'a>(entries: &'a [ZipEntry<'a>], name: &str) -> &'a [u8] {
    entries
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("missing ZIP entry {name}"))
        .data
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(
        slice(bytes, offset, offset + 2)
            .try_into()
            .expect("two-byte integer"),
    )
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        slice(bytes, offset, offset + 4)
            .try_into()
            .expect("four-byte integer"),
    )
}

fn slice(bytes: &[u8], start: usize, end: usize) -> &[u8] {
    bytes
        .get(start..end)
        .unwrap_or_else(|| panic!("ZIP field {start}..{end} is out of bounds"))
}

fn as_usize(value: u32) -> usize {
    usize::try_from(value).expect("u32 fits usize on supported targets")
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let polynomial = if crc & 1 == 0 { 0 } else { 0xedb8_8320 };
            crc = (crc >> 1) ^ polynomial;
        }
    }
    !crc
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum XmlEvent {
    Start {
        name: String,
        attributes: BTreeMap<String, String>,
        empty: bool,
    },
    End(String),
}

fn parse_xml(bytes: &[u8]) -> Vec<XmlEvent> {
    assert!(
        !bytes.starts_with(&[0xef, 0xbb, 0xbf]),
        "XML must not have a BOM"
    );
    let xml = str::from_utf8(bytes).expect("XML is UTF-8");
    let mut events = Vec::new();
    let mut stack = Vec::<String>::new();
    let mut cursor = 0;

    while cursor < xml.len() {
        let Some(relative_open) = xml[cursor..].find('<') else {
            assert!(
                xml[cursor..].chars().all(char::is_whitespace),
                "XML has trailing character data"
            );
            break;
        };
        let open = cursor + relative_open;
        assert!(
            xml[cursor..open].chars().all(char::is_whitespace),
            "unexpected XML character data"
        );

        if xml[open..].starts_with("<?") {
            let relative_end = xml[open + 2..]
                .find("?>")
                .expect("processing instruction closes");
            let declaration = &xml[open + 2..open + 2 + relative_end];
            assert!(declaration.starts_with("xml"));
            assert!(declaration.contains("version=\"1.0\""));
            assert!(declaration.contains("encoding=\"UTF-8\""));
            cursor = open + 2 + relative_end + 2;
            continue;
        }
        if xml[open..].starts_with("<!--") {
            let relative_end = xml[open + 4..].find("-->").expect("XML comment closes");
            cursor = open + 4 + relative_end + 3;
            continue;
        }
        assert!(
            !xml[open..].starts_with("<!"),
            "DTD/declaration is forbidden"
        );

        if xml[open..].starts_with("</") {
            let relative_end = xml[open + 2..].find('>').expect("end tag closes");
            let name = xml[open + 2..open + 2 + relative_end].trim();
            assert!(!name.is_empty());
            let opened = stack.pop().expect("end tag has a matching start tag");
            assert_eq!(opened, name, "XML tags are improperly nested");
            events.push(XmlEvent::End(name.to_owned()));
            cursor = open + 2 + relative_end + 1;
            continue;
        }

        let close = find_tag_close(xml, open + 1);
        let mut body = xml[open + 1..close].trim();
        let empty = body.ends_with('/');
        if empty {
            body = body[..body.len() - 1].trim_end();
        }
        let (name, attributes) = parse_start_tag(body);
        if !empty {
            stack.push(name.clone());
        }
        events.push(XmlEvent::Start {
            name,
            attributes,
            empty,
        });
        cursor = close + 1;
    }

    assert!(stack.is_empty(), "XML has unclosed tags");
    events
}

fn find_tag_close(xml: &str, start: usize) -> usize {
    let mut quote = None;
    for (relative, character) in xml[start..].char_indices() {
        match (quote, character) {
            (None, '\'' | '"') => quote = Some(character),
            (Some(open), close) if open == close => quote = None,
            (None, '>') => return start + relative,
            _ => {}
        }
    }
    panic!("XML start tag does not close")
}

fn parse_start_tag(body: &str) -> (String, BTreeMap<String, String>) {
    assert!(
        body.is_ascii(),
        "test fixture markup is expected to be ASCII"
    );
    let bytes = body.as_bytes();
    let mut cursor = 0;
    while cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    let name = body[..cursor].to_owned();
    assert!(!name.is_empty());
    let mut attributes = BTreeMap::new();

    while cursor < bytes.len() {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor == bytes.len() {
            break;
        }
        let key_start = cursor;
        while cursor < bytes.len() && !bytes[cursor].is_ascii_whitespace() && bytes[cursor] != b'='
        {
            cursor += 1;
        }
        let key = &body[key_start..cursor];
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        assert_eq!(bytes.get(cursor), Some(&b'='));
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let delimiter = *bytes.get(cursor).expect("attribute value has a quote");
        assert!(delimiter == b'\'' || delimiter == b'"');
        cursor += 1;
        let value_start = cursor;
        while cursor < bytes.len() && bytes[cursor] != delimiter {
            cursor += 1;
        }
        assert!(cursor < bytes.len(), "attribute value closes");
        let value = &body[value_start..cursor];
        cursor += 1;
        assert!(
            attributes
                .insert(key.to_owned(), value.to_owned())
                .is_none(),
            "duplicate XML attribute {key}"
        );
    }
    (name, attributes)
}

fn assert_exact_attributes(actual: &BTreeMap<String, String>, expected: &[(&str, &str)]) {
    let expected = expected
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(*actual, expected);
}

fn assert_content_types(bytes: &[u8]) {
    let events = parse_xml(bytes);
    assert_eq!(events.len(), 4);
    let XmlEvent::Start {
        name,
        attributes,
        empty,
    } = &events[0]
    else {
        panic!("content-types root is missing");
    };
    assert_eq!(name, "Types");
    assert!(!empty);
    assert_exact_attributes(attributes, &[("xmlns", CONTENT_TYPES_NAMESPACE)]);

    let expected_defaults = [
        ("rels", RELATIONSHIP_CONTENT_TYPE),
        ("model", MODEL_CONTENT_TYPE),
    ];
    for (event, (extension, content_type)) in events[1..3].iter().zip(expected_defaults) {
        let XmlEvent::Start {
            name,
            attributes,
            empty,
        } = event
        else {
            panic!("content-type Default element is missing");
        };
        assert_eq!(name, "Default");
        assert!(*empty);
        assert_exact_attributes(
            attributes,
            &[("ContentType", content_type), ("Extension", extension)],
        );
    }
    assert_eq!(events[3], XmlEvent::End("Types".to_owned()));
}

fn assert_root_relationship(bytes: &[u8]) {
    let events = parse_xml(bytes);
    assert_eq!(events.len(), 3);
    let XmlEvent::Start {
        name,
        attributes,
        empty,
    } = &events[0]
    else {
        panic!("relationships root is missing");
    };
    assert_eq!(name, "Relationships");
    assert!(!empty);
    assert_exact_attributes(attributes, &[("xmlns", RELATIONSHIPS_NAMESPACE)]);

    let XmlEvent::Start {
        name,
        attributes,
        empty,
    } = &events[1]
    else {
        panic!("StartPart relationship is missing");
    };
    assert_eq!(name, "Relationship");
    assert!(*empty);
    assert_exact_attributes(
        attributes,
        &[
            ("Id", "rel0"),
            ("Target", "/3D/3dmodel.model"),
            ("Type", START_PART_RELATIONSHIP),
        ],
    );
    assert_eq!(events[2], XmlEvent::End("Relationships".to_owned()));
}

#[derive(Debug)]
struct ParsedObject {
    id: u32,
    name: String,
    content: ParsedObjectContent,
}

#[derive(Debug)]
enum ParsedObjectContent {
    Mesh {
        vertices: Vec<[String; 3]>,
        triangles: Vec<[u32; 3]>,
    },
    Components(Vec<u32>),
}

#[derive(Debug)]
struct ParsedModel {
    objects: Vec<ParsedObject>,
    build_items: Vec<u32>,
}

#[derive(Debug)]
struct ObjectDraft {
    id: u32,
    name: String,
    content: Option<ParsedObjectContent>,
}

fn parse_model(bytes: &[u8]) -> ParsedModel {
    let events = parse_xml(bytes);
    let mut model_root_seen = false;
    let mut resources_open = false;
    let mut resources_complete = false;
    let mut build_open = false;
    let mut current_object: Option<ObjectDraft> = None;
    let mut objects = Vec::new();
    let mut build_items = Vec::new();

    for event in events {
        match event {
            XmlEvent::Start {
                name,
                attributes,
                empty,
            } => match name.as_str() {
                "model" => {
                    assert!(!model_root_seen && !empty);
                    model_root_seen = true;
                    assert_eq!(
                        attributes.get("xmlns").map(String::as_str),
                        Some(CORE_MODEL_NAMESPACE)
                    );
                    assert_eq!(
                        attributes.get("unit").map(String::as_str),
                        Some("millimeter")
                    );
                    if let Some(language) = attributes.get("xml:lang") {
                        assert_eq!(language, "en-US");
                    }
                    assert!(
                        attributes
                            .keys()
                            .all(|key| { matches!(key.as_str(), "xmlns" | "unit" | "xml:lang") })
                    );
                }
                "resources" => {
                    assert!(model_root_seen && !resources_open && !resources_complete && !empty);
                    assert!(attributes.is_empty());
                    resources_open = true;
                }
                "object" => {
                    assert!(resources_open && current_object.is_none() && !empty);
                    assert!(
                        attributes
                            .keys()
                            .all(|key| { matches!(key.as_str(), "id" | "name" | "type") })
                    );
                    assert_eq!(
                        attributes.get("type").map_or("model", String::as_str),
                        "model"
                    );
                    current_object = Some(ObjectDraft {
                        id: parse_u32_attribute(&attributes, "id"),
                        name: required_attribute(&attributes, "name").to_owned(),
                        content: None,
                    });
                }
                "mesh" => {
                    assert!(!empty && attributes.is_empty());
                    let object = current_object.as_mut().expect("mesh belongs to an object");
                    assert!(object.content.is_none());
                    object.content = Some(ParsedObjectContent::Mesh {
                        vertices: Vec::new(),
                        triangles: Vec::new(),
                    });
                }
                "vertices" | "triangles" => {
                    assert!(!empty && attributes.is_empty());
                }
                "vertex" => {
                    assert!(empty);
                    assert_exact_attribute_names(&attributes, &["x", "y", "z"]);
                    let ParsedObjectContent::Mesh { vertices, .. } =
                        current_content(&mut current_object)
                    else {
                        panic!("vertex belongs to a mesh object");
                    };
                    vertices.push([
                        required_attribute(&attributes, "x").to_owned(),
                        required_attribute(&attributes, "y").to_owned(),
                        required_attribute(&attributes, "z").to_owned(),
                    ]);
                }
                "triangle" => {
                    assert!(empty);
                    assert_exact_attribute_names(&attributes, &["v1", "v2", "v3"]);
                    let triangle = [
                        parse_u32_attribute(&attributes, "v1"),
                        parse_u32_attribute(&attributes, "v2"),
                        parse_u32_attribute(&attributes, "v3"),
                    ];
                    let ParsedObjectContent::Mesh { triangles, .. } =
                        current_content(&mut current_object)
                    else {
                        panic!("triangle belongs to a mesh object");
                    };
                    triangles.push(triangle);
                }
                "components" => {
                    assert!(!empty && attributes.is_empty());
                    let object = current_object
                        .as_mut()
                        .expect("components belong to an object");
                    assert!(object.content.is_none());
                    object.content = Some(ParsedObjectContent::Components(Vec::new()));
                }
                "component" => {
                    assert!(empty);
                    assert_exact_attribute_names(&attributes, &["objectid"]);
                    let object_id = parse_u32_attribute(&attributes, "objectid");
                    let ParsedObjectContent::Components(components) =
                        current_content(&mut current_object)
                    else {
                        panic!("component belongs to a components object");
                    };
                    components.push(object_id);
                }
                "build" => {
                    assert!(resources_complete && !build_open && !empty);
                    assert!(attributes.is_empty());
                    build_open = true;
                }
                "item" => {
                    assert!(build_open && empty);
                    assert_exact_attribute_names(&attributes, &["objectid"]);
                    build_items.push(parse_u32_attribute(&attributes, "objectid"));
                }
                other => panic!("unexpected core model element {other}"),
            },
            XmlEvent::End(name) => match name.as_str() {
                "model" | "mesh" | "vertices" | "triangles" | "components" => {}
                "resources" => {
                    assert!(resources_open && current_object.is_none());
                    resources_open = false;
                    resources_complete = true;
                }
                "object" => {
                    let object = current_object.take().expect("object start was observed");
                    objects.push(ParsedObject {
                        id: object.id,
                        name: object.name,
                        content: object.content.expect("object has mesh or components"),
                    });
                }
                "build" => build_open = false,
                other => panic!("unexpected core model end element {other}"),
            },
        }
    }

    assert!(model_root_seen && resources_complete && !resources_open && !build_open);
    ParsedModel {
        objects,
        build_items,
    }
}

fn current_content(current: &mut Option<ObjectDraft>) -> &mut ParsedObjectContent {
    current
        .as_mut()
        .expect("element belongs to an object")
        .content
        .as_mut()
        .expect("object content was established")
}

fn required_attribute<'a>(attributes: &'a BTreeMap<String, String>, name: &str) -> &'a str {
    attributes
        .get(name)
        .unwrap_or_else(|| panic!("required attribute {name} is missing"))
}

fn parse_u32_attribute(attributes: &BTreeMap<String, String>, name: &str) -> u32 {
    required_attribute(attributes, name)
        .parse()
        .unwrap_or_else(|_| panic!("attribute {name} is not a u32"))
}

fn assert_exact_attribute_names(attributes: &BTreeMap<String, String>, expected: &[&str]) {
    assert_eq!(
        attributes
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        expected.iter().copied().collect::<BTreeSet<_>>()
    );
}

fn assert_model_matches(source: &PartModel, parsed: &ParsedModel) {
    let meshes = std::iter::once(source.substrate())
        .chain(source.fill_parts())
        .collect::<Vec<_>>();
    assert_eq!(parsed.objects.len(), meshes.len() + 1);

    for (index, (mesh, object)) in meshes.iter().zip(&parsed.objects).enumerate() {
        let expected_id = u32::try_from(index + 1).expect("fixture object ID fits u32");
        let expected_name = if index == 0 {
            "substrate".to_owned()
        } else {
            format!("fill-{index:04}")
        };
        assert_eq!(object.id, expected_id);
        assert_eq!(object.name, expected_name);
        let ParsedObjectContent::Mesh {
            vertices,
            triangles,
        } = &object.content
        else {
            panic!("leaf object must contain a mesh");
        };
        assert_mesh_matches(mesh, vertices, triangles);
    }

    let assembly = parsed.objects.last().expect("assembly object exists");
    let assembly_id = u32::try_from(meshes.len() + 1).expect("fixture assembly ID fits u32");
    assert_eq!(assembly.id, assembly_id);
    assert_eq!(assembly.name, "plaque3mf-assembly");
    let ParsedObjectContent::Components(components) = &assembly.content else {
        panic!("parent object must contain components");
    };
    assert_eq!(
        *components,
        (1..assembly_id).collect::<Vec<_>>(),
        "assembly references every leaf once in canonical order"
    );
    assert_eq!(parsed.build_items, [assembly_id]);

    let ids = parsed
        .objects
        .iter()
        .map(|object| object.id)
        .collect::<Vec<_>>();
    assert_eq!(ids, (1..=assembly_id).collect::<Vec<_>>());
}

fn assert_mesh_matches(source: &TriangleMesh, vertices: &[[String; 3]], triangles: &[[u32; 3]]) {
    assert_eq!(vertices.len(), source.vertices().len());
    for (actual, expected) in vertices.iter().zip(source.vertices()) {
        assert_eq!(actual[0], canonical_millimeters(expected.x()));
        assert_eq!(actual[1], canonical_millimeters(expected.y()));
        assert_eq!(actual[2], canonical_millimeters(expected.z()));
        assert_eq!(parse_millimeters(&actual[0]), expected.x());
        assert_eq!(parse_millimeters(&actual[1]), expected.y());
        assert_eq!(parse_millimeters(&actual[2]), expected.z());
    }
    assert_eq!(triangles.len(), source.triangles().len());
    for (actual, expected) in triangles.iter().zip(source.triangles()) {
        assert_eq!(*actual, expected.indices());
        assert!(actual.iter().all(|index| as_usize(*index) < vertices.len()));
    }
}

fn canonical_millimeters(micrometers: i64) -> String {
    let sign = if micrometers < 0 { "-" } else { "" };
    let magnitude = micrometers.unsigned_abs();
    let whole = magnitude / 1_000;
    let remainder = magnitude % 1_000;
    if remainder == 0 {
        return format!("{sign}{whole}");
    }
    let fraction = format!("{remainder:03}");
    format!("{sign}{whole}.{}", fraction.trim_end_matches('0'))
}

fn parse_millimeters(value: &str) -> i64 {
    assert!(!value.contains(['e', 'E', ',']));
    let (negative, unsigned) = value
        .strip_prefix('-')
        .map_or((false, value), |unsigned| (true, unsigned));
    let mut parts = unsigned.split('.');
    let whole = parts
        .next()
        .expect("whole coordinate exists")
        .parse::<u64>()
        .expect("whole coordinate is an integer");
    let fraction = parts.next().unwrap_or("");
    assert!(parts.next().is_none() && fraction.len() <= 3);
    assert!(fraction.bytes().all(|byte| byte.is_ascii_digit()));
    let mut fractional_micrometers = fraction.parse::<u64>().unwrap_or(0);
    for _ in fraction.len()..3 {
        fractional_micrometers *= 10;
    }
    let magnitude = whole
        .checked_mul(1_000)
        .and_then(|value| value.checked_add(fractional_micrometers))
        .expect("fixture coordinate fits u64");
    let signed = i64::try_from(magnitude).expect("fixture coordinate fits i64");
    if negative { -signed } else { signed }
}

#[derive(Debug, Default)]
struct ShortWriter {
    bytes: Vec<u8>,
    maximum_chunk: usize,
}

impl Write for ShortWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let count = buffer.len().min(self.maximum_chunk);
        self.bytes.extend_from_slice(&buffer[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Default)]
struct InterruptedOnceWriter {
    bytes: Vec<u8>,
    interrupted: bool,
}

impl Write for InterruptedOnceWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if !self.interrupted {
            self.interrupted = true;
            return Err(io::Error::new(io::ErrorKind::Interrupted, "retry me"));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug, Default)]
struct WriteZeroWriter;

impl Write for WriteZeroWriter {
    fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
        Ok(0)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Debug)]
struct FailAfterWriter {
    bytes: Vec<u8>,
    limit: usize,
}

impl Write for FailAfterWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.bytes.len() == self.limit {
            return Err(io::Error::other("sentinel writer failure"));
        }
        let count = buffer.len().min(self.limit - self.bytes.len());
        self.bytes.extend_from_slice(&buffer[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn io_error_in_chain<'a>(error: &'a (dyn StdError + 'static)) -> Option<&'a io::Error> {
    let mut current = Some(error);
    while let Some(source) = current {
        if let Some(io_error) = source.downcast_ref::<io::Error>() {
            return Some(io_error);
        }
        current = source.source();
    }
    None
}

#[test]
fn crc32_matches_the_standard_check_value() {
    assert_eq!(crc32(b""), 0);
    assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
}

#[test]
fn package_is_minimal_deterministic_stored_zip32() {
    let model = multi_fill_fixture();
    let first = archive(&model);
    let same_model_again = archive(&model);
    let rebuilt_model = archive(&multi_fill_fixture());
    assert_eq!(first, same_model_again);
    assert_eq!(first, rebuilt_model);

    let entries = parse_zip(&first);
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        EXPECTED_PATHS
    );
    assert_content_types(entry(&entries, CONTENT_TYPES_PATH));
    assert_root_relationship(entry(&entries, RELATIONSHIPS_PATH));
}

#[test]
fn model_xml_preserves_exact_meshes_and_builds_one_named_assembly() {
    let source = multi_fill_fixture();
    let bytes = archive(&source);
    let entries = parse_zip(&bytes);
    let parsed = parse_model(entry(&entries, MODEL_PATH));
    assert_model_matches(&source, &parsed);
}

#[test]
fn zero_and_one_fill_models_keep_the_same_assembly_shape() {
    for source in [substrate_only_fixture(), one_fill_fixture()] {
        let bytes = archive(&source);
        let entries = parse_zip(&bytes);
        let parsed = parse_model(entry(&entries, MODEL_PATH));
        assert_model_matches(&source, &parsed);
    }
}

#[test]
fn short_and_interrupted_writes_are_retried_without_changing_bytes() {
    let model = multi_fill_fixture();
    let expected = archive(&model);
    let mut short = ShortWriter {
        bytes: Vec::new(),
        maximum_chunk: 3,
    };
    write_3mf(&model, &mut short).expect("short writes are supported");
    assert_eq!(short.bytes, expected);

    let mut interrupted = InterruptedOnceWriter::default();
    write_3mf(&model, &mut interrupted).expect("Interrupted is retried");
    assert_eq!(interrupted.bytes, expected);
}

#[test]
fn write_zero_is_returned_as_a_typed_io_source() {
    let model = substrate_only_fixture();
    let error =
        write_3mf(&model, &mut WriteZeroWriter).expect_err("a writer making no progress must fail");
    let source = io_error_in_chain(&error).expect("export error contains an io::Error source");
    assert_eq!(source.kind(), io::ErrorKind::WriteZero);
}

#[test]
fn writer_failure_is_preserved_as_the_error_source() {
    let model = multi_fill_fixture();
    let mut writer = FailAfterWriter {
        bytes: Vec::new(),
        limit: 64,
    };
    let error = write_3mf(&model, &mut writer).expect_err("sentinel writer must fail");
    assert_eq!(writer.bytes.len(), writer.limit);
    let source = io_error_in_chain(&error).expect("export error contains an io::Error source");
    assert_eq!(source.kind(), io::ErrorKind::Other);
    assert_eq!(source.to_string(), "sentinel writer failure");
    assert!(error.to_string().contains("sentinel writer failure"));
}
