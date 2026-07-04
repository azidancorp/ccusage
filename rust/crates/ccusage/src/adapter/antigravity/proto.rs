#[derive(Debug, Clone, Default)]
pub(super) struct ProtoFields {
    strings: Vec<ProtoString>,
    varints: Vec<ProtoVarint>,
}

#[derive(Debug, Clone)]
pub(super) struct ProtoString {
    pub(super) path: String,
    pub(super) value: String,
}

#[derive(Debug, Clone)]
pub(super) struct ProtoVarint {
    pub(super) path: String,
    pub(super) value: u64,
}

impl ProtoFields {
    pub(super) fn strings(&self) -> &[ProtoString] {
        &self.strings
    }

    pub(super) fn varint(&self, path: &str) -> Option<u64> {
        self.varints
            .iter()
            .find(|varint| varint.path == path)
            .map(|varint| varint.value)
    }
}

pub(super) fn parse_fields(bytes: &[u8]) -> ProtoFields {
    let mut fields = ProtoFields::default();
    parse_message(bytes, "", 0, &mut fields);
    fields
}

fn parse_message(bytes: &[u8], prefix: &str, depth: usize, fields: &mut ProtoFields) {
    if depth > 8 {
        return;
    }
    let mut offset = 0;
    while offset < bytes.len() {
        let Some(key) = read_varint(bytes, &mut offset) else {
            return;
        };
        if key == 0 {
            return;
        }
        let field = key >> 3;
        let wire_type = key & 7;
        let path = field_path(prefix, field);
        match wire_type {
            0 => {
                let Some(value) = read_varint(bytes, &mut offset) else {
                    return;
                };
                fields.varints.push(ProtoVarint { path, value });
            }
            1 => {
                if !skip(bytes, &mut offset, 8) {
                    return;
                }
            }
            2 => {
                let Some(length) = read_varint(bytes, &mut offset) else {
                    return;
                };
                let Ok(length) = usize::try_from(length) else {
                    return;
                };
                if offset + length > bytes.len() {
                    return;
                }
                let payload = &bytes[offset..offset + length];
                offset += length;
                let string = decode_printable_string(payload);
                if let Some(value) = string.as_ref() {
                    fields.strings.push(ProtoString {
                        path: path.clone(),
                        value: value.clone(),
                    });
                }
                if string.as_deref().is_none_or(|value| value.contains('\0')) {
                    parse_message(payload, &path, depth + 1, fields);
                }
            }
            5 => {
                if !skip(bytes, &mut offset, 4) {
                    return;
                }
            }
            _ => return,
        }
    }
}

fn field_path(prefix: &str, field: u64) -> String {
    if prefix.is_empty() {
        field.to_string()
    } else {
        format!("{prefix}.{field}")
    }
}

fn read_varint(bytes: &[u8], offset: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    let mut shift = 0;
    while *offset < bytes.len() && shift < 64 {
        let byte = bytes[*offset];
        *offset += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
        shift += 7;
    }
    None
}

fn skip(bytes: &[u8], offset: &mut usize, length: usize) -> bool {
    if *offset + length > bytes.len() {
        return false;
    }
    *offset += length;
    true
}

fn decode_printable_string(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    let value = std::str::from_utf8(bytes).ok()?;
    let printable = value
        .chars()
        .filter(|character| matches!(character, '\n' | '\r' | '\t') || !character.is_control())
        .count();
    let total = value.chars().count();
    if total == 0 || printable * 100 < total * 85 {
        return None;
    }
    Some(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(field: u64, wire_type: u64) -> Vec<u8> {
        varint((field << 3) | wire_type)
    }

    fn varint(mut value: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        loop {
            let mut byte = (value & 0x7f) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            bytes.push(byte);
            if value == 0 {
                break;
            }
        }
        bytes
    }

    fn string_field(field: u64, value: &str) -> Vec<u8> {
        let mut bytes = key(field, 2);
        bytes.extend(varint(value.len() as u64));
        bytes.extend(value.as_bytes());
        bytes
    }

    fn message_field(field: u64, payload: Vec<u8>) -> Vec<u8> {
        let mut bytes = key(field, 2);
        bytes.extend(varint(payload.len() as u64));
        bytes.extend(payload);
        bytes
    }

    #[test]
    fn extracts_nested_string_paths() {
        let bytes = message_field(19, string_field(2, "hello"));

        let fields = parse_fields(&bytes);

        assert_eq!(fields.strings()[0].path, "19.2");
        assert_eq!(fields.strings()[0].value, "hello");
    }

    #[test]
    fn extracts_nested_varints() {
        let mut timestamp = key(1, 0);
        timestamp.extend(varint(1_782_468_596));
        let bytes = message_field(5, message_field(1, timestamp));

        let fields = parse_fields(&bytes);

        assert_eq!(fields.varint("5.1.1"), Some(1_782_468_596));
    }
}
