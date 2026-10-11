use super::*;
pub(super) fn kinds() -> Value {
    json!([
     {"kind":"icc-profile","mediaTypes":["application/vnd.iccprofile"],"maximumBytes":MAX_RESOURCE,"availability":available(),"valueSchema":{"type":"object","required":["resourceId","version"],"additionalProperties":false,"properties":{"resourceId":{"type":"string"},"version":{"type":"string"}}},"frames":[]},
     {"kind":"lut","mediaTypes":["application/x-cube"],"maximumBytes":MAX_RESOURCE,"availability":available(),"valueSchema":{"type":"object","required":["resourceId","version"],"additionalProperties":false,"properties":{"resourceId":{"type":"string"},"version":{"type":"string"}}},"frames":[]},
     {"kind":"raster-mask","mediaTypes":["image/png","image/tiff"],"maximumBytes":MAX_RESOURCE,"maximumWidth":16384,"maximumHeight":16384,"maximumPixels":67108864,"availability":available(),"valueSchema":{"type":"object","required":["resourceId","version"],"additionalProperties":false,"properties":{"resourceId":{"type":"string"},"version":{"type":"string"}}},"frames":[]}
    ])
}
pub(super) fn import(args: &Value) -> Result<(Value, Option<Vec<u8>>), ToolError> {
    let path = Path::new(text(args, "file")?);
    let bytes = read_file(path, MAX_RESOURCE)?;
    let kind = text(args, "kind")?;
    let facts = validate(kind, &bytes)?;
    let record = json!({"id":"pending","version":"pending","kind":kind,"name":path.file_name().and_then(|v|v.to_str()).unwrap_or("resource"),"mediaType":facts["mediaType"],"sha256":digest(&bytes),"byteLength":bytes.len(),"createdAt":"pending","availability":available(),"references":{"currentEdits":0,"retainedRevisions":0,"catalogs":0,"tasks":0},"facts":facts});
    Ok((record, Some(bytes)))
}
fn invalid(message: &str) -> ToolError {
    error("invalid_resource", message)
}
pub(super) fn validate(kind: &str, bytes: &[u8]) -> Result<Value, ToolError> {
    if bytes.is_empty() || bytes.len() > MAX_RESOURCE {
        return Err(invalid("resource byte bound exceeded"));
    }
    match kind {
        "icc-profile" => icc(bytes),
        "lut" => cube(bytes),
        "raster-mask" => raster(bytes),
        _ => Err(invalid(
            "unknown typed resource kind; native plugins/state are not resources",
        )),
    }
}
fn be(bytes: &[u8], at: usize) -> Result<u32, ToolError> {
    bytes
        .get(at..at + 4)
        .map(|v| u32::from_be_bytes(v.try_into().unwrap()))
        .ok_or_else(|| invalid("truncated ICC field"))
}
fn icc(bytes: &[u8]) -> Result<Value, ToolError> {
    if bytes.len() < 132 || bytes.get(36..40) != Some(b"acsp") {
        return Err(invalid("ICC signature missing"));
    }
    if be(bytes, 0)? as usize != bytes.len() {
        return Err(invalid("ICC declared size differs from file bytes"));
    }
    let major = bytes[8];
    if ![2, 4].contains(&major) {
        return Err(invalid("unsupported ICC version"));
    }
    if ![
        b"scnr", b"mntr", b"prtr", b"spac", b"abst", b"link", b"nmcl",
    ]
    .iter()
    .any(|v| bytes.get(12..16) == Some(v.as_slice()))
    {
        return Err(invalid("invalid ICC device class"));
    }
    if bytes.get(20..24) != Some(b"XYZ ") && bytes.get(20..24) != Some(b"Lab ") {
        return Err(invalid("invalid ICC connection space"));
    }
    let count = be(bytes, 128)? as usize;
    if count == 0
        || count > 4096
        || 132usize
            .checked_add(count * 12)
            .filter(|n| *n <= bytes.len())
            .is_none()
    {
        return Err(invalid("invalid ICC tag table"));
    }
    let mut names = std::collections::BTreeSet::new();
    for n in 0..count {
        let at = 132 + n * 12;
        let sig = &bytes[at..at + 4];
        if !names.insert(sig) {
            return Err(invalid("duplicate ICC tag"));
        }
        let start = be(bytes, at + 4)? as usize;
        let size = be(bytes, at + 8)? as usize;
        if start < 132 + count * 12
            || !start.is_multiple_of(4)
            || size < 8
            || start
                .checked_add(size)
                .filter(|end| *end <= bytes.len())
                .is_none()
        {
            return Err(invalid("ICC tag exceeds profile bounds"));
        }
        if bytes[start + 4..start + 8] != [0; 4] {
            return Err(invalid("ICC tag reserved bytes must be zero"));
        }
    }
    Ok(
        json!({"mediaType":"application/vnd.iccprofile","iccVersion":major,"colorSpace":String::from_utf8_lossy(&bytes[16..20]),"connectionSpace":String::from_utf8_lossy(&bytes[20..24]),"tagCount":count}),
    )
}
fn cube(bytes: &[u8]) -> Result<Value, ToolError> {
    let source = std::str::from_utf8(bytes).map_err(|_| invalid("LUT must be UTF-8 cube text"))?;
    if source.contains('\0') {
        return Err(invalid("NUL is invalid in LUT"));
    }
    let mut size = None;
    let mut count = 0usize;
    let mut domain_min = [0.0; 3];
    let mut domain_max = [1.0; 3];
    let mut min_seen = false;
    let mut max_seen = false;
    let mut title = false;
    for line in source.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        let first = fields.next().unwrap();
        match first {
            "TITLE" => {
                if title || count > 0 || !line[5..].trim().starts_with('"') || !line.ends_with('"')
                {
                    return Err(invalid("invalid LUT title"));
                }
                title = true;
            }
            "LUT_3D_SIZE" | "LUT_1D_SIZE" => {
                if size.is_some() || count > 0 {
                    return Err(invalid("duplicate or misplaced LUT size"));
                }
                let n = fields
                    .next()
                    .and_then(|s| s.parse::<usize>().ok())
                    .ok_or_else(|| invalid("invalid LUT size"))?;
                if fields.next().is_some()
                    || n < 2
                    || n > if first == "LUT_3D_SIZE" { 256 } else { 65536 }
                {
                    return Err(invalid("LUT dimension bound exceeded"));
                }
                size = Some((n, first == "LUT_3D_SIZE"));
            }
            "DOMAIN_MIN" | "DOMAIN_MAX" => {
                if count > 0 {
                    return Err(invalid("domain after LUT samples"));
                }
                let vals = numbers(fields)?;
                if first == "DOMAIN_MIN" {
                    if min_seen {
                        return Err(invalid("duplicate LUT domain"));
                    }
                    min_seen = true;
                    domain_min = vals;
                } else {
                    if max_seen {
                        return Err(invalid("duplicate LUT domain"));
                    }
                    max_seen = true;
                    domain_max = vals;
                }
            }
            _ => {
                if size.is_none() {
                    return Err(invalid("LUT samples precede size"));
                }
                numbers(line.split_whitespace())?;
                count += 1;
            }
        }
    }
    let (n, three) = size.ok_or_else(|| invalid("missing LUT size"))?;
    let expected = if three { n * n * n } else { n };
    if count != expected
        || domain_min
            .iter()
            .zip(domain_max)
            .any(|(min, max)| *min >= max)
    {
        return Err(invalid("LUT sample count or domain invalid"));
    }
    Ok(
        json!({"mediaType":"application/x-cube","dimension":if three{3}else{1},"size":n,"domainMin":domain_min,"domainMax":domain_max}),
    )
}
fn numbers<'a>(values: impl Iterator<Item = &'a str>) -> Result<[f64; 3], ToolError> {
    let v: Vec<_> = values.collect();
    if v.len() != 3 {
        return Err(invalid("LUT rows require three finite numbers"));
    }
    let mut out = [0.0_f64; 3];
    for (i, s) in v.iter().enumerate() {
        out[i] = s.parse().map_err(|_| invalid("invalid LUT number"))?;
        if !out[i].is_finite() {
            return Err(invalid("LUT numbers must be finite"));
        }
    }
    Ok(out)
}
fn raster(bytes: &[u8]) -> Result<Value, ToolError> {
    use image::ImageDecoder;
    let format =
        image::guess_format(bytes).map_err(|_| invalid("raster mask is not PNG or TIFF"))?;
    if !matches!(format, image::ImageFormat::Png | image::ImageFormat::Tiff) {
        return Err(invalid("raster mask is not PNG or TIFF"));
    }
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16384);
    limits.max_image_height = Some(16384);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let decoder = reader
        .into_decoder()
        .map_err(|e| invalid(&format!("invalid raster structure: {e}")))?;
    let (w, h) = decoder.dimensions();
    if w == 0
        || h == 0
        || u64::from(w) * u64::from(h) > 67108864
        || decoder.total_bytes() > 256 * 1024 * 1024
    {
        return Err(invalid("raster dimensions or decoded bytes exceed limits"));
    }
    let color = decoder.color_type();
    if !matches!(
        color,
        image::ColorType::L8
            | image::ColorType::L16
            | image::ColorType::La8
            | image::ColorType::La16
    ) {
        return Err(invalid("raster mask must contain grayscale samples"));
    }
    let mut decoded = vec![0; decoder.total_bytes() as usize];
    decoder
        .read_image(&mut decoded)
        .map_err(|e| invalid(&format!("invalid raster pixels: {e}")))?;
    Ok(
        json!({"mediaType":if format==image::ImageFormat::Png{"image/png"}else{"image/tiff"},"width":w,"height":h,"sampleType":format!("{color:?}"),"decodedBytes":decoded.len()}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cube_rejects_infinite_samples_and_wrong_count() {
        assert!(validate("lut", b"LUT_1D_SIZE 2\n0 0 0\ninf 1 1\n").is_err());
        assert!(validate("lut", b"LUT_3D_SIZE 2\n0 0 0\n").is_err());
        assert!(validate("lut", b"LUT_1D_SIZE 2\n0 0 0\n1 1 1\n").is_ok());
    }
    #[test]
    fn native_blob_cannot_be_resource() {
        assert!(validate("plugin", b"native").is_err());
        assert!(validate("icc-profile", b"acsp").is_err());
    }
}
