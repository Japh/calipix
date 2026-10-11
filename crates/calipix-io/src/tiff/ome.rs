//! The small part of OME-XML needed to open a single-file OME-TIFF: the
//! dimensions, plane order, physical sizes and channel names of the first
//! image. Field meanings follow the OME data model as implemented by SCIFIO's
//! `OMETIFFFormat`.

use super::ifd::{TiffResult, unsupported};

/// What the first `Image` element of an OME-XML block says.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct OmeImage {
    pub size_c: usize,
    pub size_z: usize,
    pub size_t: usize,
    /// The three letters after "XY" in DimensionOrder, fastest first.
    pub order: [char; 3],
    pub physical_x: Option<(f64, String)>,
    pub physical_y: Option<(f64, String)>,
    pub physical_z: Option<(f64, String)>,
    pub time_increment: Option<(f64, String)>,
    pub channel_names: Vec<Option<String>>,
}

/// True if an ImageDescription holds OME-XML.
pub(crate) fn is_ome_xml(description: &str) -> bool {
    let text = description.trim_start_matches('\u{feff}').trim_start();
    text.starts_with('<') && tags(text).any(|t| !t.closing && t.local == "OME")
}

struct TagRef<'a> {
    local: &'a str,
    closing: bool,
    attributes: &'a str,
}

/// Iterates over element tags, skipping comments, declarations and CDATA.
fn tags(xml: &str) -> impl Iterator<Item = TagRef<'_>> {
    let mut rest = xml;
    std::iter::from_fn(move || {
        loop {
            let start = rest.find('<')?;
            rest = &rest[start..];
            let special = [("<!--", "-->"), ("<![CDATA[", "]]>"), ("<?", "?>")]
                .into_iter()
                .find(|(open, _)| rest.starts_with(open));
            if let Some((_, close)) = special {
                let end = rest.find(close)?;
                rest = &rest[end + close.len()..];
                continue;
            }
            let end = rest.find('>')?;
            let inner = &rest[1..end];
            rest = &rest[end + 1..];
            if inner.starts_with('!') {
                continue;
            }
            let (closing, inner) = match inner.strip_prefix('/') {
                Some(inner) => (true, inner),
                None => (false, inner),
            };
            let name_end = inner
                .find(|c: char| c.is_whitespace() || c == '/')
                .unwrap_or(inner.len());
            let name = &inner[..name_end];
            let local = name.rsplit(':').next().unwrap_or(name);
            return Some(TagRef {
                local,
                closing,
                attributes: &inner[name_end..],
            });
        }
    })
}

/// The value of attribute `name` in the attribute text of a start tag.
fn attribute(attributes: &str, name: &str) -> Option<String> {
    let mut rest = attributes;
    loop {
        let eq = rest.find('=')?;
        let key = rest[..eq].trim().trim_end_matches('/');
        let key = key.rsplit(char::is_whitespace).next().unwrap_or(key);
        let after = rest[eq + 1..].trim_start();
        let quote = after.chars().next().filter(|c| matches!(c, '"' | '\''))?;
        let end = after[1..].find(quote)?;
        let value = &after[1..1 + end];
        if key == name {
            return Some(unescape_xml(value));
        }
        rest = &after[end + 2..];
    }
}

fn unescape_xml(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest.find(';').filter(|&s| s <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .map(|hex| u32::from_str_radix(hex, 16))
                .or_else(|| entity.strip_prefix('#').map(str::parse))
                .and_then(Result::ok)
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Reads the first image of an OME-XML block. `file_name` is the name of
/// the TIFF being opened, used to recognise datasets that span other files.
///
/// Returns `Ok(None)` if the XML has no usable `Pixels` element, in which
/// case the file is opened as a plain TIFF.
pub(crate) fn parse(xml: &str, file_name: &str) -> TiffResult<Option<OmeImage>> {
    let mut image: Option<OmeImage> = None;
    let mut in_first_pixels = false;
    let mut images_seen = 0;
    for tag in tags(xml) {
        match (tag.local, tag.closing) {
            ("BinaryOnly", false) => {
                let other = attribute(tag.attributes, "MetadataFile").unwrap_or_default();
                return unsupported(format!(
                    "multi-file OME-TIFF datasets are not supported yet (the metadata is in \"{other}\"; open that file instead)"
                ));
            }
            ("Image", false) => images_seen += 1,
            ("Pixels", false) if images_seen == 1 && image.is_none() => {
                let size = |name: &str| {
                    attribute(tag.attributes, name)
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .filter(|&v| v > 0)
                };
                let (Some(size_c), Some(size_z), Some(size_t)) =
                    (size("SizeC"), size("SizeZ"), size("SizeT"))
                else {
                    return Ok(None);
                };
                let order_text =
                    attribute(tag.attributes, "DimensionOrder").unwrap_or("XYCZT".to_string());
                let letters: Vec<char> = order_text.chars().skip(2).collect();
                let mut sorted = letters.clone();
                sorted.sort_unstable();
                if !order_text.starts_with("XY") || sorted != ['C', 'T', 'Z'] {
                    return Ok(None);
                }
                let physical = |name: &str, default_unit: &str| {
                    let value: f64 = attribute(tag.attributes, name)?.trim().parse().ok()?;
                    let unit = attribute(tag.attributes, &format!("{name}Unit"))
                        .unwrap_or(default_unit.to_string());
                    (value.is_finite() && value > 0.0).then_some((value, unit))
                };
                image = Some(OmeImage {
                    size_c,
                    size_z,
                    size_t,
                    order: [letters[0], letters[1], letters[2]],
                    physical_x: physical("PhysicalSizeX", "\u{b5}m"),
                    physical_y: physical("PhysicalSizeY", "\u{b5}m"),
                    physical_z: physical("PhysicalSizeZ", "\u{b5}m"),
                    time_increment: physical("TimeIncrement", "s"),
                    channel_names: Vec::new(),
                });
                in_first_pixels = !tag.attributes.trim_end().ends_with('/');
            }
            ("Pixels", true) => in_first_pixels = false,
            ("Channel", false) if in_first_pixels => {
                if let Some(image) = image.as_mut() {
                    image
                        .channel_names
                        .push(attribute(tag.attributes, "Name").filter(|n| !n.is_empty()));
                }
            }
            // A UUID element inside TiffData names the file holding those
            // planes; any name but our own means a multi-file dataset.
            ("UUID", false) if in_first_pixels => {
                if let Some(other) = attribute(tag.attributes, "FileName") {
                    let base = other.rsplit(['/', '\\']).next().unwrap_or(&other);
                    if !base.is_empty() && !file_name.is_empty() && base != file_name {
                        return unsupported(format!(
                            "multi-file OME-TIFF datasets are not supported yet (this one also needs \"{other}\")"
                        ));
                    }
                }
            }
            _ => {}
        }
    }
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?><!-- generated -->
<OME xmlns="http://www.openmicroscopy.org/Schemas/OME/2016-06">
  <Image ID="Image:0" Name="first">
    <Pixels ID="Pixels:0" DimensionOrder="XYZCT" Type="uint16" SizeX="8" SizeY="6" SizeZ="3" SizeC="2" SizeT="1"
            PhysicalSizeX="0.25" PhysicalSizeY="0.5" PhysicalSizeZ="1.5" PhysicalSizeZUnit="nm" TimeIncrement="2">
      <Channel ID="Channel:0:0" Name="DAPI &amp; more" SamplesPerPixel="1"/>
      <Channel ID="Channel:0:1" SamplesPerPixel="1"/>
      <TiffData IFD="0" PlaneCount="6"><UUID FileName="a.ome.tif">urn:uuid:1</UUID></TiffData>
    </Pixels>
  </Image>
  <Image ID="Image:1"><Pixels SizeC="9" SizeZ="9" SizeT="9" DimensionOrder="XYCZT"><Channel Name="other"/></Pixels></Image>
</OME>"#;

    #[test]
    fn reads_first_image() {
        assert!(is_ome_xml(XML));
        assert!(!is_ome_xml("ImageJ=1.54p\n"));
        assert!(!is_ome_xml("<html><body>OME</body></html>"));
        let image = parse(XML, "a.ome.tif").unwrap().unwrap();
        assert_eq!((image.size_c, image.size_z, image.size_t), (2, 3, 1));
        assert_eq!(image.order, ['Z', 'C', 'T']);
        assert_eq!(image.physical_x, Some((0.25, "\u{b5}m".to_string())));
        assert_eq!(image.physical_z, Some((1.5, "nm".to_string())));
        assert_eq!(image.time_increment, Some((2.0, "s".to_string())));
        assert_eq!(
            image.channel_names,
            vec![Some("DAPI & more".to_string()), None]
        );
    }

    #[test]
    fn multi_file_datasets_are_refused() {
        let err = parse(XML, "b.ome.tif").unwrap_err();
        assert!(err.to_string().contains("multi-file"), "{err}");
        let binary_only = r#"<OME><BinaryOnly MetadataFile="main.ome.tif" UUID="x"/></OME>"#;
        let err = parse(binary_only, "b.ome.tif").unwrap_err();
        assert!(err.to_string().contains("main.ome.tif"), "{err}");
    }

    #[test]
    fn incomplete_xml_is_not_fatal() {
        assert_eq!(parse("<OME><Image><Pixels SizeC='1'/>", "x").unwrap(), None);
        assert_eq!(parse("<OME><Image><Pixels", "x").unwrap(), None);
        assert_eq!(parse("", "x").unwrap(), None);
        for cut in 0..XML.len() {
            if XML.is_char_boundary(cut) {
                let _ = parse(&XML[..cut], "a.ome.tif");
            }
        }
    }

    #[test]
    fn entities_decode() {
        assert_eq!(
            unescape_xml("a&lt;b &#xB5;m &#181; &bogus; &"),
            "a<b µm µ &bogus; &"
        );
    }
}
