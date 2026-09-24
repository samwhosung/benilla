//! A WDBC reader for WoW 1.12.1: a 20-byte header, `record_count × record_size` bytes of 4-byte
//! fields, then a string block. The file has no column types; the caller supplies a [`Schema`].

use std::io::{Cursor, Write};

use benilla_bytes::{capped, ByteExt};

/// Column type for a DBC field. Every field is 4 bytes on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    UInt32,
    Int32,
    Float32,
    /// A `u32` offset into the string block.
    String,
}

/// One schema field; a `count` above 1 is an inline array of that many 4-byte slots.
#[derive(Debug, Clone)]
pub struct SchemaField {
    pub name: String,
    pub ty: FieldType,
    pub count: usize,
}

impl SchemaField {
    pub fn new(name: impl Into<String>, ty: FieldType) -> Self {
        Self {
            name: name.into(),
            ty,
            count: 1,
        }
    }

    pub fn new_array(name: impl Into<String>, ty: FieldType, count: usize) -> Self {
        Self {
            name: name.into(),
            ty,
            count,
        }
    }
}

/// A hand-supplied description of a DBC's columns (the file has none).
#[derive(Debug, Clone, Default)]
pub struct Schema {
    pub name: String,
    pub fields: Vec<SchemaField>,
    pub key_field: Option<String>,
}

impl Schema {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            fields: Vec::new(),
            key_field: None,
        }
    }

    pub fn add_field(&mut self, field: SchemaField) {
        self.fields.push(field);
    }

    pub fn set_key_field(&mut self, name: impl Into<String>) {
        self.key_field = Some(name.into());
    }

    /// The 4-byte slots covered, arrays expanded; the header's `field_count` must match.
    fn expanded_len(&self) -> usize {
        self.fields.iter().map(|f| f.count).sum()
    }
}

/// A `u32` offset into a record set's string block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StringRef(pub u32);

/// One decoded field value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    UInt32(u32),
    Int32(i32),
    Float32(f32),
    StringRef(StringRef),
}

/// One decoded record: a flat list of [`Value`]s, one per expanded field slot.
#[derive(Debug, Clone)]
pub struct Record {
    values: Vec<Value>,
}

impl Record {
    /// The value at expanded slot `i` (arrays count as `count` consecutive slots).
    pub fn get_value(&self, i: usize) -> Option<&Value> {
        self.values.get(i)
    }
}

/// The 20-byte WDBC header.
#[derive(Debug, Clone, Copy)]
pub struct Header {
    pub record_count: u32,
    pub field_count: u32,
    pub record_size: u32,
    pub string_block_size: u32,
}

/// Errors from DBC parsing.
#[derive(Debug)]
pub enum Error {
    NotWdbc,
    Truncated(&'static str),
    /// Schema's expanded field count doesn't match the file's `field_count`.
    SchemaFieldMismatch {
        schema: usize,
        file: u32,
    },
    BadStringRef(u32),
    /// `record_count × record_size + string_block_size` overflows `usize`: a corrupt header.
    SizeOverflow,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotWdbc => write!(f, "not a WDBC file (bad magic)"),
            Error::Truncated(what) => write!(f, "truncated DBC: {what}"),
            Error::SchemaFieldMismatch { schema, file } => {
                write!(f, "schema has {schema} fields but file has {file}")
            }
            Error::BadStringRef(off) => write!(f, "string ref {off} out of bounds"),
            Error::SizeOverflow => {
                write!(
                    f,
                    "record_count * record_size + string_block_size overflows usize"
                )
            }
        }
    }
}

impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;

/// A little-endian u32 whose bytes past the end of `b` read as zero, unlike [`ByteExt::u32_at`].
fn rd_u32_at(b: &[u8], o: usize) -> u32 {
    let mut bytes = [0u8; 4];
    for (k, slot) in bytes.iter_mut().enumerate() {
        if let Some(v) = o.checked_add(k).and_then(|idx| b.u8_at(idx)) {
            *slot = v;
        }
    }
    u32::from_le_bytes(bytes)
}

/// `(record bytes, record bytes + string block)`, checked: the header's counts are unvalidated.
fn checked_body_layout(
    record_count: usize,
    record_size: usize,
    string_block_size: usize,
) -> Option<(usize, usize)> {
    let record_bytes_len = record_count.checked_mul(record_size)?;
    let total = record_bytes_len.checked_add(string_block_size)?;
    Some((record_bytes_len, total))
}

/// A DBC opened for reading; a [`Schema`] is attached before decoding records.
pub struct DbcParser<'a> {
    header: Header,
    /// Everything after the 20-byte header: the record bytes, then the string block.
    body: &'a [u8],
    schema: Option<Schema>,
}

impl<'a> DbcParser<'a> {
    /// Parse the header of a whole DBC; the cursor's position is ignored.
    pub fn parse(cursor: &mut Cursor<&'a [u8]>) -> Result<Self> {
        let all: &'a [u8] = cursor.get_ref();
        if all.len() < 20 || &all[0..4] != b"WDBC" {
            return Err(Error::NotWdbc);
        }
        let header = Header {
            record_count: all.u32_at(4).ok_or(Error::Truncated("header"))?,
            field_count: all.u32_at(8).ok_or(Error::Truncated("header"))?,
            record_size: all.u32_at(12).ok_or(Error::Truncated("header"))?,
            string_block_size: all.u32_at(16).ok_or(Error::Truncated("header"))?,
        };
        let body = all.get(20..).ok_or(Error::Truncated("body"))?;
        let (_, need) = checked_body_layout(
            header.record_count as usize,
            header.record_size as usize,
            header.string_block_size as usize,
        )
        .ok_or(Error::SizeOverflow)?;
        if body.len() < need {
            return Err(Error::Truncated("records + string block"));
        }
        Ok(Self {
            header,
            body,
            schema: None,
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Attach a schema, validating its expanded field count against the file header.
    pub fn with_schema(mut self, schema: Schema) -> Result<Self> {
        let n = schema.expanded_len();
        if n != self.header.field_count as usize {
            return Err(Error::SchemaFieldMismatch {
                schema: n,
                file: self.header.field_count,
            });
        }
        self.schema = Some(schema);
        Ok(self)
    }

    /// Decode all records into an owned [`RecordSet`] (records + a copy of the string block).
    pub fn parse_records(&self) -> Result<RecordSet> {
        let schema = self.schema.as_ref().ok_or(Error::Truncated("no schema"))?;
        let rc = self.header.record_count as usize;
        let rs = self.header.record_size as usize;
        let fc = self.header.field_count as usize;
        let (record_bytes_len, strings_end) =
            checked_body_layout(rc, rs, self.header.string_block_size as usize)
                .ok_or(Error::SizeOverflow)?;
        let records_bytes = self
            .body
            .get(..record_bytes_len)
            .ok_or(Error::Truncated("records"))?;
        let strings = self
            .body
            .get(record_bytes_len..strings_end)
            .ok_or(Error::Truncated("string block"))?
            .to_vec();

        // One type and one column name per 4-byte slot, arrays expanded.
        let mut types = Vec::with_capacity(fc);
        let mut names = Vec::with_capacity(fc);
        for field in &schema.fields {
            for k in 0..field.count {
                types.push(field.ty);
                names.push(if field.count == 1 {
                    field.name.clone()
                } else {
                    format!("{}[{k}]", field.name)
                });
            }
        }

        // A `record_size` of 0 passes the size guard for any count and `rd_u32_at` never fails, so
        // records claimed at size 0 are refused and the loop runs over what the bytes hold.
        if rs == 0 && rc > 0 {
            return Err(Error::Truncated("records claimed at record_size 0"));
        }
        let rc = capped(rc, rs, records_bytes.len());
        let mut records = Vec::with_capacity(rc);
        for r in 0..rc {
            let base = r * rs;
            let mut values = Vec::with_capacity(fc);
            for (i, ty) in types.iter().enumerate() {
                let raw = rd_u32_at(records_bytes, base + i * 4);
                values.push(match ty {
                    FieldType::UInt32 => Value::UInt32(raw),
                    FieldType::Int32 => Value::Int32(raw as i32),
                    FieldType::Float32 => Value::Float32(f32::from_bits(raw)),
                    FieldType::String => Value::StringRef(StringRef(raw)),
                });
            }
            records.push(Record { values });
        }

        Ok(RecordSet {
            field_names: names,
            records,
            strings,
        })
    }
}

/// A fully-decoded DBC: records plus the string block they reference.
pub struct RecordSet {
    field_names: Vec<String>,
    records: Vec<Record>,
    strings: Vec<u8>,
}

impl RecordSet {
    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// Resolve a [`StringRef`] to its NUL-terminated string in the block (UTF-8, lossy).
    pub fn get_string(&self, r: StringRef) -> Result<std::borrow::Cow<'_, str>> {
        let off = r.0 as usize;
        if off > self.strings.len() {
            return Err(Error::BadStringRef(r.0));
        }
        let end = self.strings[off..]
            .iter()
            .position(|&b| b == 0)
            .map(|p| off + p)
            .unwrap_or(self.strings.len());
        Ok(String::from_utf8_lossy(&self.strings[off..end]))
    }
}

/// Write a record set as RFC-4180 CSV: column names, then one row per record, strings resolved.
pub fn export_to_csv<W: Write>(rs: &RecordSet, mut w: W) -> std::io::Result<()> {
    writeln!(w, "{}", rs.field_names.join(","))?;
    for record in &rs.records {
        let cells: Vec<String> = record
            .values
            .iter()
            .map(|v| match v {
                Value::UInt32(x) => x.to_string(),
                Value::Int32(x) => x.to_string(),
                Value::Float32(x) => x.to_string(),
                Value::StringRef(sr) => csv_quote(&rs.get_string(*sr).unwrap_or_default()),
            })
            .collect();
        writeln!(w, "{}", cells.join(","))?;
    }
    Ok(())
}

fn csv_quote(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A WDBC: the 20-byte header, the record bytes, the string block.
    fn build_wdbc(
        record_count: u32,
        field_count: u32,
        record_size: u32,
        records: &[u8],
        strings: &[u8],
    ) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"WDBC");
        b.extend_from_slice(&record_count.to_le_bytes());
        b.extend_from_slice(&field_count.to_le_bytes());
        b.extend_from_slice(&record_size.to_le_bytes());
        b.extend_from_slice(&(strings.len() as u32).to_le_bytes());
        b.extend_from_slice(records);
        b.extend_from_slice(strings);
        b
    }

    fn id_name_schema() -> Schema {
        let mut s = Schema::new("Test");
        s.add_field(SchemaField::new("id", FieldType::UInt32));
        s.add_field(SchemaField::new("name", FieldType::String));
        s
    }

    #[test]
    fn parses_minimal_synthetic_wdbc() {
        let strings = b"Alice\0Bob\0";
        let mut records = Vec::new();
        records.extend_from_slice(&1u32.to_le_bytes()); // id
        records.extend_from_slice(&0u32.to_le_bytes()); // name -> "Alice" @0
        records.extend_from_slice(&2u32.to_le_bytes()); // id
        records.extend_from_slice(&6u32.to_le_bytes()); // name -> "Bob" @6
        let bytes = build_wdbc(2, 2, 8, &records, strings);

        let parser = DbcParser::parse(&mut Cursor::new(bytes.as_slice())).expect("parses");
        assert_eq!(parser.header().record_count, 2);
        assert_eq!(parser.header().field_count, 2);

        let rs = parser
            .with_schema(id_name_schema())
            .expect("schema matches header field_count")
            .parse_records()
            .expect("decodes");
        assert_eq!(rs.records().len(), 2);

        let r0 = &rs.records()[0];
        assert_eq!(r0.get_value(0), Some(&Value::UInt32(1)));
        let Some(Value::StringRef(name0)) = r0.get_value(1).copied() else {
            panic!("expected a string ref");
        };
        assert_eq!(rs.get_string(name0).unwrap(), "Alice");

        let r1 = &rs.records()[1];
        assert_eq!(r1.get_value(0), Some(&Value::UInt32(2)));
        let Some(Value::StringRef(name1)) = r1.get_value(1).copied() else {
            panic!("expected a string ref");
        };
        assert_eq!(rs.get_string(name1).unwrap(), "Bob");
    }

    #[test]
    fn rd_u32_at_zero_extends_short_tail() {
        let b = [0xAA, 0xBB];
        assert_eq!(rd_u32_at(&b, 0), 0x0000_BBAA);
        assert_eq!(rd_u32_at(&b, 1), 0x0000_00BB);
        assert_eq!(rd_u32_at(&b, 2), 0); // fully past the end
        assert_eq!(rd_u32_at(&b, 5), 0); // offset itself already past the end
    }

    #[test]
    fn header_size_arithmetic_overflow_errors_cleanly_not_panic() {
        // u32 header fields overflow only a 32-bit usize, so the arithmetic is driven directly.
        assert_eq!(checked_body_layout(usize::MAX, 2, 0), None);
        assert_eq!(checked_body_layout(4, 4, usize::MAX), None);
        assert_eq!(checked_body_layout(usize::MAX, 1, 1), None);
        assert_eq!(checked_body_layout(10, 4, 6), Some((40, 46)));

        // Through the public API, an all-zero header parses.
        let bytes = build_wdbc(0, 0, 0, &[], &[]);
        assert!(DbcParser::parse(&mut Cursor::new(bytes.as_slice())).is_ok());
    }

    #[test]
    fn truncated_body_errors_cleanly_not_panic() {
        // The header claims 2 records of 8 bytes; the body holds one.
        let bytes = build_wdbc(2, 2, 8, &[0u8; 8], &[]);
        let err = DbcParser::parse(&mut Cursor::new(bytes.as_slice()))
            .err()
            .unwrap();
        assert!(matches!(err, Error::Truncated("records + string block")));
    }

    #[test]
    fn hostile_shapes_do_not_panic() {
        assert!(matches!(
            DbcParser::parse(&mut Cursor::new(&[][..])),
            Err(Error::NotWdbc)
        ));
        assert!(matches!(
            DbcParser::parse(&mut Cursor::new(&[0u8; 19][..])), // one short of the 20-byte header
            Err(Error::NotWdbc)
        ));
        let mut bad_magic = vec![0u8; 20];
        bad_magic[0..4].copy_from_slice(b"XXXX");
        assert!(matches!(
            DbcParser::parse(&mut Cursor::new(bad_magic.as_slice())),
            Err(Error::NotWdbc)
        ));
    }

    #[test]
    fn schema_field_count_must_match_header() {
        let bytes = build_wdbc(1, 2, 8, &[0u8; 8], &[]);
        let parser = DbcParser::parse(&mut Cursor::new(bytes.as_slice())).unwrap();
        let mut wide = Schema::new("Wide");
        wide.add_field(SchemaField::new("a", FieldType::UInt32));
        wide.add_field(SchemaField::new("b", FieldType::UInt32));
        wide.add_field(SchemaField::new("c", FieldType::UInt32));
        assert!(matches!(
            parser.with_schema(wide),
            Err(Error::SchemaFieldMismatch { schema: 3, file: 2 })
        ));
    }

    #[test]
    fn array_field_expands_to_consecutive_slots() {
        let mut records = Vec::new();
        records.extend_from_slice(&7u32.to_le_bytes()); // id
        records.extend_from_slice(&10u32.to_le_bytes()); // coords[0]
        records.extend_from_slice(&20u32.to_le_bytes()); // coords[1]
        records.extend_from_slice(&30u32.to_le_bytes()); // coords[2]
        let bytes = build_wdbc(1, 4, 16, &records, &[]);

        let mut schema = Schema::new("Arr");
        schema.add_field(SchemaField::new("id", FieldType::UInt32));
        schema.add_field(SchemaField::new_array("coords", FieldType::UInt32, 3));

        let rs = DbcParser::parse(&mut Cursor::new(bytes.as_slice()))
            .unwrap()
            .with_schema(schema)
            .unwrap()
            .parse_records()
            .unwrap();
        assert_eq!(
            rs.field_names,
            ["id", "coords[0]", "coords[1]", "coords[2]"]
        );
        let r = &rs.records()[0];
        assert_eq!(r.get_value(0), Some(&Value::UInt32(7)));
        assert_eq!(r.get_value(1), Some(&Value::UInt32(10)));
        assert_eq!(r.get_value(3), Some(&Value::UInt32(30)));
        assert_eq!(r.get_value(4), None); // past the last slot
    }

    #[test]
    fn int32_and_float32_fields_decode() {
        let mut records = Vec::new();
        records.extend_from_slice(&(-5i32).to_le_bytes());
        records.extend_from_slice(&1.5f32.to_bits().to_le_bytes());
        let bytes = build_wdbc(1, 2, 8, &records, &[]);

        let mut schema = Schema::new("Nums");
        schema.add_field(SchemaField::new("signed", FieldType::Int32));
        schema.add_field(SchemaField::new("ratio", FieldType::Float32));

        let rs = DbcParser::parse(&mut Cursor::new(bytes.as_slice()))
            .unwrap()
            .with_schema(schema)
            .unwrap()
            .parse_records()
            .unwrap();
        let r = &rs.records()[0];
        assert_eq!(r.get_value(0), Some(&Value::Int32(-5)));
        assert_eq!(r.get_value(1), Some(&Value::Float32(1.5)));
    }

    #[test]
    fn get_string_handles_bad_ref_and_unterminated_tail() {
        let strings = b"hi\0tail"; // "tail" has no trailing NUL
        let bytes = build_wdbc(1, 1, 4, &0u32.to_le_bytes(), strings);
        let rs = {
            let mut s = Schema::new("S");
            s.add_field(SchemaField::new("name", FieldType::String));
            DbcParser::parse(&mut Cursor::new(bytes.as_slice()))
                .unwrap()
                .with_schema(s)
                .unwrap()
                .parse_records()
                .unwrap()
        };
        assert_eq!(rs.get_string(StringRef(0)).unwrap(), "hi");
        assert_eq!(rs.get_string(StringRef(3)).unwrap(), "tail");
        // An offset equal to the length is the empty string; past it errors.
        assert_eq!(rs.get_string(StringRef(strings.len() as u32)).unwrap(), "");
        assert!(matches!(
            rs.get_string(StringRef(strings.len() as u32 + 1)),
            Err(Error::BadStringRef(_))
        ));
    }

    #[test]
    fn csv_export_resolves_strings_and_quotes_per_rfc4180() {
        let strings = b"plain\0a,b\0he said \"hi\"\0line1\nline2\0";
        // offsets: plain@0, "a,b"@6, 'he said "hi"'@10, "line1\nline2"@23
        let mut records = Vec::new();
        for (id, off) in [(1u32, 0u32), (2, 6), (3, 10), (4, 23)] {
            records.extend_from_slice(&id.to_le_bytes());
            records.extend_from_slice(&off.to_le_bytes());
        }
        let bytes = build_wdbc(4, 2, 8, &records, strings);
        let rs = DbcParser::parse(&mut Cursor::new(bytes.as_slice()))
            .unwrap()
            .with_schema(id_name_schema())
            .unwrap()
            .parse_records()
            .unwrap();

        let mut out = Vec::new();
        export_to_csv(&rs, &mut out).unwrap();
        let csv = String::from_utf8(out).unwrap();
        let expected = "id,name\n\
             1,plain\n\
             2,\"a,b\"\n\
             3,\"he said \"\"hi\"\"\"\n\
             4,\"line1\nline2\"\n";
        assert_eq!(csv, expected);
    }

    /// A 20-byte file claiming `u32::MAX` records of size 0 must not run the loop that many times.
    #[test]
    fn zero_record_size_with_records_is_refused_not_iterated() {
        let bytes = build_wdbc(u32::MAX, 2, 0, &[], &[]);
        let parser = DbcParser::parse(&mut Cursor::new(bytes.as_slice()))
            .expect("0 × anything fits the size guard")
            .with_schema(id_name_schema())
            .expect("schema matches");
        assert!(matches!(parser.parse_records(), Err(Error::Truncated(_))));
        // No records at size 0 still decodes, to nothing.
        let bytes = build_wdbc(0, 2, 0, &[], &[]);
        let rs = DbcParser::parse(&mut Cursor::new(bytes.as_slice()))
            .unwrap()
            .with_schema(id_name_schema())
            .unwrap()
            .parse_records()
            .unwrap();
        assert!(rs.records().is_empty());
    }
}
