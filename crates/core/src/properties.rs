use std::{collections::BTreeMap, io::Read, path::Path};

use quick_xml::{Reader, events::Event};

use crate::PropertiesError;

const MAX_PROPERTIES_BYTES: u64 = 1024 * 1024;

/// String-valued JUCE-compatible `PROPERTIES` document.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct PropertiesFile {
    values: BTreeMap<String, String>,
}

impl std::fmt::Debug for PropertiesFile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted: BTreeMap<_, _> = self
            .values
            .keys()
            .map(|name| (name, "[redacted]"))
            .collect();
        formatter
            .debug_struct("PropertiesFile")
            .field("values", &redacted)
            .finish()
    }
}

impl PropertiesFile {
    /// Reads a bounded properties file. A missing file is an empty document.
    ///
    /// # Errors
    ///
    /// Returns I/O or malformed-XML errors without modifying the source file.
    pub fn read(path: &Path) -> Result<Self, PropertiesError> {
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(PropertiesError::Io(error)),
        };
        let mut text = String::new();
        file.take(MAX_PROPERTIES_BYTES + 1)
            .read_to_string(&mut text)?;
        if text.len() as u64 > MAX_PROPERTIES_BYTES {
            return Err(PropertiesError::TooLarge);
        }
        Self::from_xml(&text)
    }

    /// Parses a `PROPERTIES` document with `VALUE name="…" value="…"` entries.
    ///
    /// # Errors
    ///
    /// Returns [`PropertiesError::Malformed`] for invalid XML, roots, entries, or nesting.
    pub fn from_xml(xml: &str) -> Result<Self, PropertiesError> {
        let mut reader = Reader::from_str(xml);
        reader.config_mut().trim_text(true);
        let mut values = BTreeMap::new();
        let mut depth = 0_usize;
        let mut saw_root = false;
        loop {
            match reader.read_event() {
                Ok(Event::Start(element)) => {
                    depth += 1;
                    if depth > 16 {
                        return Err(PropertiesError::Malformed);
                    }
                    if element.name().as_ref() == "PROPERTIES" {
                        saw_root = true;
                    }
                }
                Ok(Event::End(_)) => {
                    depth = depth.checked_sub(1).ok_or(PropertiesError::Malformed)?;
                }
                Ok(Event::Empty(element)) if element.name().as_ref() == "VALUE" => {
                    let mut name = None;
                    let mut value = None;
                    for attribute in element.attributes().with_checks(false) {
                        let attribute = attribute.map_err(|_| PropertiesError::Malformed)?;
                        match attribute.key.as_ref() {
                            "name" => {
                                name = Some(
                                    attribute
                                        .normalized_value(quick_xml::XmlVersion::Explicit1_0)
                                        .map_err(|_| PropertiesError::Malformed)?
                                        .into_owned(),
                                );
                            }
                            "value" => {
                                value = Some(
                                    attribute
                                        .normalized_value(quick_xml::XmlVersion::Explicit1_0)
                                        .map_err(|_| PropertiesError::Malformed)?
                                        .into_owned(),
                                );
                            }
                            _ => {}
                        }
                    }
                    let name = name.ok_or(PropertiesError::Malformed)?;
                    let value = value.ok_or(PropertiesError::Malformed)?;
                    values.insert(name, value);
                }
                Ok(Event::Eof) => break,
                Ok(_) => {}
                Err(_) => return Err(PropertiesError::Malformed),
            }
        }
        if !saw_root {
            return Err(PropertiesError::Malformed);
        }
        Ok(Self { values })
    }

    /// Serializes a deterministic JUCE-compatible XML document.
    #[must_use]
    pub fn to_xml(&self) -> String {
        let mut xml = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<PROPERTIES>\n");
        for (name, value) in &self.values {
            xml.push_str("  <VALUE name=\"");
            xml.push_str(&escape(name));
            xml.push_str("\" value=\"");
            xml.push_str(&escape(value));
            xml.push_str("\"/>\n");
        }
        xml.push_str("</PROPERTIES>\n");
        xml
    }

    /// Sets one string value.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        self.values.insert(name.into(), value.into());
    }

    /// Sets a boolean using the Reference `true`/`false` spelling.
    pub fn set_bool(&mut self, name: impl Into<String>, value: bool) {
        self.set(name, if value { "true" } else { "false" });
    }

    /// Removes one key.
    pub fn remove(&mut self, name: &str) {
        self.values.remove(name);
    }

    /// Returns a borrowed string value.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// Reads the exact `true`/`false` spellings with a fallback.
    #[must_use]
    pub fn get_bool(&self, name: &str, fallback: bool) -> bool {
        match self.get(name) {
            Some("true") => true,
            Some("false") => false,
            _ => fallback,
        }
    }
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
