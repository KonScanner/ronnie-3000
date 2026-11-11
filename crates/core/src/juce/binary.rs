use crate::CodecError;

use super::{JuceTree, JuceValue};

const TYPE_INT: u8 = 1;
const TYPE_TRUE: u8 = 2;
const TYPE_FALSE: u8 = 3;
const TYPE_DOUBLE: u8 = 4;
const TYPE_STRING: u8 = 5;
const TYPE_INT64: u8 = 6;
const TYPE_ARRAY: u8 = 7;
const TYPE_BINARY: u8 = 8;
const TYPE_UNDEFINED: u8 = 9;

/// Bounded binary codec for JUCE variant and `ValueTree` streams.
#[derive(Debug, Clone, Copy)]
pub struct JuceCodec {
    /// Maximum tree/array nesting depth, including the root.
    pub max_depth: usize,
    /// Maximum total tree and array nodes.
    pub max_nodes: usize,
    /// Maximum binary or string payload size.
    pub max_blob_bytes: usize,
}

impl Default for JuceCodec {
    fn default() -> Self {
        Self {
            max_depth: 64,
            max_nodes: 100_000,
            max_blob_bytes: 50 * 1024 * 1024,
        }
    }
}

impl JuceCodec {
    /// Decodes one complete tree and rejects trailing bytes.
    ///
    /// # Errors
    ///
    /// Returns framing, UTF-8, type, count, depth, node, blob, or trailing-data errors.
    pub fn read_tree(&self, input: &[u8]) -> Result<JuceTree, CodecError> {
        let (tree, consumed) = self.read_tree_prefix(input)?;
        if consumed != input.len() {
            return Err(CodecError::TrailingBytes);
        }
        Ok(tree)
    }

    /// Decodes one tree prefix and returns its exact consumed length for framed formats.
    ///
    /// # Errors
    ///
    /// Returns framing, UTF-8, type, count, depth, node, or blob errors.
    pub fn read_tree_prefix(&self, input: &[u8]) -> Result<(JuceTree, usize), CodecError> {
        let mut reader = Reader::new(input, self);
        let tree = reader.tree(1)?;
        Ok((tree, reader.position))
    }

    /// Encodes one tree with JUCE's deterministic property/child order.
    ///
    /// # Errors
    ///
    /// Returns depth, node, count, blob, or allocation errors.
    pub fn write_tree(&self, tree: &JuceTree) -> Result<Vec<u8>, CodecError> {
        let mut writer = Writer::new(self);
        writer.tree(tree, 1)?;
        Ok(writer.bytes)
    }
}

struct Reader<'a> {
    input: &'a [u8],
    position: usize,
    codec: &'a JuceCodec,
    nodes: usize,
}

impl<'a> Reader<'a> {
    fn new(input: &'a [u8], codec: &'a JuceCodec) -> Self {
        Self {
            input,
            position: 0,
            codec,
            nodes: 0,
        }
    }

    fn tree(&mut self, depth: usize) -> Result<JuceTree, CodecError> {
        self.enter_node(depth)?;
        let identifier = self.string()?;
        if identifier.is_empty() {
            return Err(CodecError::InvalidValue("empty tree identifier"));
        }
        let property_count = self.count()?;
        self.check_count(property_count)?;
        let mut properties = Vec::new();
        properties
            .try_reserve_exact(property_count)
            .map_err(|_| CodecError::Allocation)?;
        for _ in 0..property_count {
            let name = self.string()?;
            if name.is_empty() {
                return Err(CodecError::InvalidValue("empty property name"));
            }
            properties.push((name, self.value(depth)?));
        }
        let child_count = self.count()?;
        self.check_count(child_count)?;
        let mut children = Vec::new();
        children
            .try_reserve_exact(child_count)
            .map_err(|_| CodecError::Allocation)?;
        for _ in 0..child_count {
            children.push(self.tree(depth + 1)?);
        }
        Ok(JuceTree {
            identifier,
            properties,
            children,
        })
    }

    fn value(&mut self, depth: usize) -> Result<JuceValue, CodecError> {
        self.enter_node(depth)?;
        let byte_count = self.count()?;
        if byte_count == 0 {
            return Ok(JuceValue::Void);
        }
        let kind = self.byte()?;
        let payload = byte_count - 1;
        match kind {
            TYPE_INT => {
                self.expect_payload(payload, 4)?;
                Ok(JuceValue::Int(self.i32()?))
            }
            TYPE_INT64 => {
                self.expect_payload(payload, 8)?;
                Ok(JuceValue::Int64(self.i64()?))
            }
            TYPE_TRUE => {
                self.expect_payload(payload, 0)?;
                Ok(JuceValue::Bool(true))
            }
            TYPE_FALSE => {
                self.expect_payload(payload, 0)?;
                Ok(JuceValue::Bool(false))
            }
            TYPE_DOUBLE => {
                self.expect_payload(payload, 8)?;
                Ok(JuceValue::Double(self.double()?))
            }
            TYPE_STRING => {
                if payload == 0 || payload > self.codec.max_blob_bytes {
                    return Err(CodecError::BlobTooLarge {
                        actual: payload,
                        max: self.codec.max_blob_bytes,
                    });
                }
                Ok(JuceValue::String(self.string()?))
            }
            TYPE_BINARY => {
                if payload > self.codec.max_blob_bytes {
                    return Err(CodecError::BlobTooLarge {
                        actual: payload,
                        max: self.codec.max_blob_bytes,
                    });
                }
                Ok(JuceValue::Binary(self.take(payload)?.to_vec()))
            }
            TYPE_ARRAY => {
                let payload_bytes = self.take(payload)?;
                let mut nested = Reader::new(payload_bytes, self.codec);
                nested.nodes = self.nodes;
                let count = nested.count()?;
                nested.check_count(count)?;
                let mut values = Vec::new();
                values
                    .try_reserve_exact(count)
                    .map_err(|_| CodecError::Allocation)?;
                for _ in 0..count {
                    values.push(nested.value(depth + 1)?);
                }
                if nested.position != payload_bytes.len() {
                    return Err(CodecError::InvalidValue("array payload has trailing bytes"));
                }
                self.nodes = nested.nodes;
                Ok(JuceValue::Array(values))
            }
            TYPE_UNDEFINED => {
                self.expect_payload(payload, 0)?;
                Ok(JuceValue::Undefined)
            }
            other => Err(CodecError::UnsupportedType(other)),
        }
    }

    fn enter_node(&mut self, depth: usize) -> Result<(), CodecError> {
        if depth > self.codec.max_depth {
            return Err(CodecError::DepthLimit {
                max: self.codec.max_depth,
            });
        }
        self.nodes = self.nodes.checked_add(1).ok_or(CodecError::NodeLimit {
            max: self.codec.max_nodes,
        })?;
        if self.nodes > self.codec.max_nodes {
            return Err(CodecError::NodeLimit {
                max: self.codec.max_nodes,
            });
        }
        Ok(())
    }

    fn check_count(&self, count: usize) -> Result<(), CodecError> {
        if count > self.codec.max_nodes {
            return Err(CodecError::NodeLimit {
                max: self.codec.max_nodes,
            });
        }
        Ok(())
    }

    fn expect_payload(&self, actual: usize, expected: usize) -> Result<(), CodecError> {
        if actual != expected {
            return Err(CodecError::InvalidValue("variant payload length mismatch"));
        }
        if self.position + actual > self.input.len() {
            return Err(CodecError::Truncated);
        }
        Ok(())
    }

    fn count(&mut self) -> Result<usize, CodecError> {
        let value = self.compressed_int()?;
        if value < 0 {
            return Err(CodecError::InvalidCount);
        }
        usize::try_from(value).map_err(|_| CodecError::InvalidCount)
    }

    fn compressed_int(&mut self) -> Result<i32, CodecError> {
        let header = self.byte()?;
        let count = usize::from(header & 0x7f);
        if count > 4 {
            return Err(CodecError::InvalidCount);
        }
        if count == 0 {
            return Ok(0);
        }
        let mut value = 0_u32;
        for index in 0..count {
            let shift = u32::try_from(index * 8).map_err(|_| CodecError::InvalidCount)?;
            value |= u32::from(self.byte()?)
                .checked_shl(shift)
                .ok_or(CodecError::InvalidCount)?;
        }
        if header & 0x80 != 0 {
            let signed = -i64::from(value);
            i32::try_from(signed).map_err(|_| CodecError::InvalidCount)
        } else {
            i32::try_from(value).map_err(|_| CodecError::InvalidCount)
        }
    }

    fn string(&mut self) -> Result<String, CodecError> {
        let remaining = self.input.len() - self.position;
        let scan = remaining.min(self.codec.max_blob_bytes.saturating_add(1));
        let end = self.input[self.position..self.position + scan]
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(CodecError::Truncated)?;
        let bytes = self.take(end + 1)?;
        std::str::from_utf8(&bytes[..end])
            .map(str::to_owned)
            .map_err(|_| CodecError::InvalidUtf8)
    }

    fn i32(&mut self) -> Result<i32, CodecError> {
        Ok(i32::from_le_bytes(
            self.take(4)?
                .try_into()
                .map_err(|_| CodecError::Truncated)?,
        ))
    }

    fn i64(&mut self) -> Result<i64, CodecError> {
        Ok(i64::from_le_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| CodecError::Truncated)?,
        ))
    }

    fn double(&mut self) -> Result<f64, CodecError> {
        Ok(f64::from_le_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| CodecError::Truncated)?,
        ))
    }

    fn byte(&mut self) -> Result<u8, CodecError> {
        Ok(self.take(1)?[0])
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], CodecError> {
        let end = self
            .position
            .checked_add(count)
            .ok_or(CodecError::Truncated)?;
        if end > self.input.len() {
            return Err(CodecError::Truncated);
        }
        let bytes = &self.input[self.position..end];
        self.position = end;
        Ok(bytes)
    }
}

struct Writer<'a> {
    bytes: Vec<u8>,
    codec: &'a JuceCodec,
    nodes: usize,
}

impl<'a> Writer<'a> {
    fn new(codec: &'a JuceCodec) -> Self {
        Self {
            bytes: Vec::new(),
            codec,
            nodes: 0,
        }
    }

    fn tree(&mut self, tree: &JuceTree, depth: usize) -> Result<(), CodecError> {
        self.enter_node(depth)?;
        if tree.identifier.is_empty() {
            return Err(CodecError::InvalidValue("empty tree identifier"));
        }
        self.string(&tree.identifier)?;
        self.check_count(tree.properties.len())?;
        self.compressed_int(tree.properties.len())?;
        for (name, value) in &tree.properties {
            if name.is_empty() {
                return Err(CodecError::InvalidValue("empty property name"));
            }
            self.string(name)?;
            self.value(value, depth)?;
        }
        self.check_count(tree.children.len())?;
        self.compressed_int(tree.children.len())?;
        for child in &tree.children {
            self.tree(child, depth + 1)?;
        }
        Ok(())
    }

    fn value(&mut self, value: &JuceValue, depth: usize) -> Result<(), CodecError> {
        self.enter_node(depth)?;
        match value {
            JuceValue::Void => self.compressed_int(0),
            JuceValue::Undefined => {
                self.compressed_int(1)?;
                self.byte(TYPE_UNDEFINED);
                Ok(())
            }
            JuceValue::Int(value) => {
                self.compressed_int(5)?;
                self.byte(TYPE_INT);
                self.reserve(4)?;
                self.bytes.extend_from_slice(&value.to_le_bytes());
                Ok(())
            }
            JuceValue::Int64(value) => {
                self.compressed_int(9)?;
                self.byte(TYPE_INT64);
                self.reserve(8)?;
                self.bytes.extend_from_slice(&value.to_le_bytes());
                Ok(())
            }
            JuceValue::Bool(value) => {
                self.compressed_int(1)?;
                self.byte(if *value { TYPE_TRUE } else { TYPE_FALSE });
                Ok(())
            }
            JuceValue::Double(value) => {
                self.compressed_int(9)?;
                self.byte(TYPE_DOUBLE);
                self.reserve(8)?;
                self.bytes.extend_from_slice(&value.to_le_bytes());
                Ok(())
            }
            JuceValue::String(value) => {
                if value.len() > self.codec.max_blob_bytes {
                    return Err(CodecError::BlobTooLarge {
                        actual: value.len(),
                        max: self.codec.max_blob_bytes,
                    });
                }
                self.compressed_int(value.len().checked_add(2).ok_or(CodecError::InvalidCount)?)?;
                self.byte(TYPE_STRING);
                self.string(value)
            }
            JuceValue::Array(values) => {
                self.check_count(values.len())?;
                let mut nested = Writer::new(self.codec);
                nested.nodes = self.nodes;
                nested.compressed_int(values.len())?;
                for value in values {
                    nested.value(value, depth + 1)?;
                }
                self.nodes = nested.nodes;
                self.compressed_int(
                    nested
                        .bytes
                        .len()
                        .checked_add(1)
                        .ok_or(CodecError::InvalidCount)?,
                )?;
                self.byte(TYPE_ARRAY);
                self.reserve(nested.bytes.len())?;
                self.bytes.extend_from_slice(&nested.bytes);
                Ok(())
            }
            JuceValue::Binary(value) => {
                if value.len() > self.codec.max_blob_bytes {
                    return Err(CodecError::BlobTooLarge {
                        actual: value.len(),
                        max: self.codec.max_blob_bytes,
                    });
                }
                self.compressed_int(value.len().checked_add(1).ok_or(CodecError::InvalidCount)?)?;
                self.byte(TYPE_BINARY);
                self.reserve(value.len())?;
                self.bytes.extend_from_slice(value);
                Ok(())
            }
        }
    }

    fn enter_node(&mut self, depth: usize) -> Result<(), CodecError> {
        if depth > self.codec.max_depth {
            return Err(CodecError::DepthLimit {
                max: self.codec.max_depth,
            });
        }
        self.nodes = self.nodes.checked_add(1).ok_or(CodecError::NodeLimit {
            max: self.codec.max_nodes,
        })?;
        if self.nodes > self.codec.max_nodes {
            return Err(CodecError::NodeLimit {
                max: self.codec.max_nodes,
            });
        }
        Ok(())
    }

    fn check_count(&self, count: usize) -> Result<(), CodecError> {
        if count > self.codec.max_nodes || count > i32::MAX as usize {
            return Err(CodecError::InvalidCount);
        }
        Ok(())
    }

    fn string(&mut self, value: &str) -> Result<(), CodecError> {
        if value.len() > self.codec.max_blob_bytes {
            return Err(CodecError::BlobTooLarge {
                actual: value.len(),
                max: self.codec.max_blob_bytes,
            });
        }
        self.reserve(value.len() + 1)?;
        self.bytes.extend_from_slice(value.as_bytes());
        self.bytes.push(0);
        Ok(())
    }

    fn compressed_int(&mut self, value: usize) -> Result<(), CodecError> {
        if value > i32::MAX as usize {
            return Err(CodecError::InvalidCount);
        }
        let mut encoded = [0_u8; 5];
        let mut count = 0_usize;
        let mut remaining = value;
        while remaining > 0 {
            count += 1;
            encoded[count] = remaining as u8;
            remaining >>= 8;
        }
        encoded[0] = count as u8;
        self.reserve(count + 1)?;
        self.bytes.extend_from_slice(&encoded[..=count]);
        Ok(())
    }

    fn byte(&mut self, value: u8) {
        self.bytes.push(value);
    }

    fn reserve(&mut self, count: usize) -> Result<(), CodecError> {
        self.bytes
            .try_reserve(count)
            .map_err(|_| CodecError::Allocation)
    }
}
