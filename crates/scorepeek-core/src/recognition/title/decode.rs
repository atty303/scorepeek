//! Registered CTC dictionary parsing shared by the live text runtime.

const MAX_INFERENCE_YML_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug)]
pub enum CatalogTitleDecoderError {
    InvalidDictionary,
}

impl std::fmt::Display for CatalogTitleDecoderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDictionary => formatter.write_str("catalog title dictionary is invalid"),
        }
    }
}

impl std::error::Error for CatalogTitleDecoderError {}

pub(super) fn load_dictionary_contract(
    bytes: &[u8],
    expected_sha256: &str,
    output_classes: usize,
) -> Result<Vec<String>, CatalogTitleDecoderError> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_INFERENCE_YML_BYTES {
        return Err(CatalogTitleDecoderError::InvalidDictionary);
    }
    if crate::recognition::screen::encode_sha256(bytes) != expected_sha256 {
        return Err(CatalogTitleDecoderError::InvalidDictionary);
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| CatalogTitleDecoderError::InvalidDictionary)?;
    let marker = "  character_dict:\n";
    let (_, body) = text
        .split_once(marker)
        .ok_or(CatalogTitleDecoderError::InvalidDictionary)?;
    let mut dictionary = Vec::with_capacity(output_classes);
    dictionary.push("blank".to_owned());
    for line in body.lines() {
        let Some(value) = line.strip_prefix("  - ") else {
            return Err(CatalogTitleDecoderError::InvalidDictionary);
        };
        dictionary.push(parse_yaml_scalar(value)?);
    }
    dictionary.push(" ".to_owned());
    if dictionary.len() != output_classes {
        return Err(CatalogTitleDecoderError::InvalidDictionary);
    }
    Ok(dictionary)
}

fn parse_yaml_scalar(value: &str) -> Result<String, CatalogTitleDecoderError> {
    let decoded = if let Some(inner) = value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
        inner.replace("''", "'")
    } else {
        value.to_owned()
    };
    if decoded.is_empty() || decoded.chars().any(char::is_control) {
        return Err(CatalogTitleDecoderError::InvalidDictionary);
    }
    Ok(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dictionary_scalar_rejects_empty_and_control_characters() {
        assert!(parse_yaml_scalar("''").is_err());
        assert!(parse_yaml_scalar("bad\nvalue").is_err());
        assert_eq!(parse_yaml_scalar("'it''s'").unwrap(), "it's");
    }
}
