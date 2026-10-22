//! Lossless XML codec for XMP packets: bounded tree parsing and byte-stable serialization.
use quick_xml::{
    XmlVersion,
    escape::unescape,
    events::{BytesStart, Event},
    reader::Reader,
};
use std::collections::BTreeMap;

use super::{
    LangAltValue, MAX_DEPTH, MAX_ITEMS, MAX_LANGUAGES, MAX_NODES, MAX_TEXT,
    MAXIMUM_XMP_PACKET_BYTES, META, RDF, XML, XmpParseError,
};

#[derive(Clone, Debug)]
pub(super) enum Node {
    Element(Element),
    Text(String),
    CData(String),
    Comment(String),
    Pi(String),
    UnresolvedReference(String),
}
#[derive(Clone, Debug)]
pub(super) struct Element {
    pub(super) name: String,
    pub(super) attrs: Vec<(String, String)>,
    pub(super) children: Vec<Node>,
}
pub(super) type Namespaces = BTreeMap<String, String>;

pub(super) fn scope(parent: &Namespaces, element: &Element) -> Namespaces {
    let mut ns = parent.clone();
    for (name, value) in &element.attrs {
        if name == "xmlns" {
            ns.insert(String::new(), value.clone());
        } else if let Some(prefix) = name.strip_prefix("xmlns:") {
            ns.insert(prefix.into(), value.clone());
        }
    }
    ns
}
pub(super) fn expanded<'a>(
    name: &'a str,
    ns: &'a Namespaces,
    attribute: bool,
) -> Option<(&'a str, &'a str)> {
    if let Some((prefix, local)) = name.split_once(':') {
        if prefix == "xml" {
            return Some((XML, local));
        }
        ns.get(prefix).map(|uri| (uri.as_str(), local))
    } else if attribute {
        Some(("", name))
    } else {
        ns.get("").map(|uri| (uri.as_str(), name))
    }
}
pub(super) fn is_element(e: &Element, ns: &Namespaces, uri: &str, local: &str) -> bool {
    expanded(&e.name, ns, false) == Some((uri, local))
}
pub(super) fn attribute<'a>(
    e: &'a Element,
    ns: &Namespaces,
    uri: &str,
    local: &str,
) -> Option<&'a str> {
    e.attrs
        .iter()
        .find(|(name, _)| expanded(name, ns, true) == Some((uri, local)))
        .map(|(_, value)| value.as_str())
}
fn parse_element(start: &BytesStart<'_>, version: XmlVersion) -> Result<Element, XmpParseError> {
    let name = start.name().as_ref().to_owned();
    let mut attrs = Vec::new();
    for attr in start.attributes().with_checks(true) {
        let attr = attr.map_err(|_| XmpParseError::Malformed)?;
        let key = attr.key.as_ref().to_owned();
        let value = attr
            .normalized_value(version)
            .map_err(|_| XmpParseError::Malformed)?
            .into_owned();
        if value.len() > MAX_TEXT {
            return Err(XmpParseError::ResourceLimit);
        }
        attrs.push((key, value));
    }
    Ok(Element {
        name,
        attrs,
        children: Vec::new(),
    })
}
fn append(stack: &mut [Element], roots: &mut Vec<Node>, node: Node) {
    if let Some(parent) = stack.last_mut() {
        if let (Some(Node::Text(previous)), Node::Text(value)) = (parent.children.last_mut(), &node)
        {
            previous.push_str(value);
        } else {
            parent.children.push(node);
        }
    } else {
        roots.push(node);
    }
}
fn check_node(
    node: &Element,
    ns: &Namespaces,
    item_count: &mut usize,
    lang_count: &mut usize,
) -> Result<bool, XmpParseError> {
    let text_bytes: usize = node
        .children
        .iter()
        .filter_map(|child| match child {
            Node::Text(value) | Node::CData(value) => Some(value.len()),
            _ => None,
        })
        .sum();
    if text_bytes > MAX_TEXT {
        return Err(XmpParseError::ResourceLimit);
    }
    let mut preservable = true;
    for (name, value) in &node.attrs {
        if let Some((uri, local)) = expanded(name, ns, true) {
            if uri == RDF
                && (matches!(local, "ID" | "nodeID" | "bagID")
                    || local == "parseType" && value != "Resource"
                    || local == "about" && (value.starts_with('#') || value.contains("xpointer(")))
            {
                preservable = false;
            }
        } else if !name.starts_with("xmlns:") && name != "xmlns" {
            preservable = false;
        }
    }
    // Unbound names cannot be interpreted, but structurally malformed XML is
    // diagnosed by the event reader before preservation rules apply.
    if expanded(&node.name, ns, false).is_none() && node.name.contains(':') {
        preservable = false;
    }
    if is_element(node, ns, RDF, "li") {
        *item_count += 1;
        if *item_count > MAX_ITEMS {
            return Err(XmpParseError::ResourceLimit);
        }
        if attribute(node, ns, XML, "lang").is_some() {
            *lang_count += 1;
            if *lang_count > MAX_LANGUAGES {
                return Err(XmpParseError::ResourceLimit);
            }
        }
    }
    Ok(preservable)
}
fn escape_text(value: &str, attribute: bool, out: &mut String) {
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '\r' if attribute => out.push_str("&#13;"),
            '\n' if attribute => out.push_str("&#10;"),
            '\t' if attribute => out.push_str("&#9;"),
            _ => out.push(c),
        }
    }
}
fn serialize(node: &Node, out: &mut String) {
    match node {
        Node::Text(t) => escape_text(t, false, out),
        Node::CData(t) => {
            out.push_str("<![CDATA[");
            out.push_str(t);
            out.push_str("]]>");
        }
        Node::Comment(t) => {
            out.push_str("<!--");
            out.push_str(t);
            out.push_str("-->");
        }
        Node::Pi(t) => {
            out.push_str("<?");
            out.push_str(t);
            out.push_str("?>");
        }
        Node::UnresolvedReference(reference) => {
            out.push('&');
            out.push_str(reference);
            out.push(';');
        }
        Node::Element(e) => {
            out.push('<');
            out.push_str(&e.name);
            for (key, value) in &e.attrs {
                out.push(' ');
                out.push_str(key);
                out.push_str("=\"");
                escape_text(value, true, out);
                out.push('"');
            }
            if e.children.is_empty() {
                out.push_str("/>");
            } else {
                out.push('>');
                for child in &e.children {
                    serialize(child, out);
                }
                out.push_str("</");
                out.push_str(&e.name);
                out.push('>');
            }
        }
    }
}
pub(super) fn text(e: &Element) -> Option<String> {
    let mut value = String::new();
    for child in &e.children {
        match child {
            Node::Text(s) | Node::CData(s) => value.push_str(s),
            _ => return None,
        }
    }
    Some(value)
}
pub(super) fn single_container<'a>(
    e: &'a Element,
    ns: &Namespaces,
    local: &str,
) -> Option<(&'a Element, Namespaces)> {
    let mut found = None;
    for child in &e.children {
        match child {
            Node::Element(inner) if found.is_none() => found = Some(inner),
            Node::Text(s) if s.trim().is_empty() => (),
            Node::Element(_) | Node::CData(_) | Node::Text(_) => return None,
            _ => (),
        }
    }
    let inner = found?;
    let inner_ns = scope(ns, inner);
    is_element(inner, &inner_ns, RDF, local).then_some((inner, inner_ns))
}
pub(super) fn items<'a>(
    container: &'a Element,
    ns: &Namespaces,
) -> Option<Vec<(&'a Element, Namespaces)>> {
    let mut result = Vec::new();
    for child in &container.children {
        match child {
            Node::Element(li) => {
                let li_ns = scope(ns, li);
                if !is_element(li, &li_ns, RDF, "li") {
                    return None;
                }
                result.push((li, li_ns));
            }
            Node::Text(s) if s.trim().is_empty() => (),
            Node::Comment(_) | Node::Pi(_) => (),
            _ => return None,
        }
    }
    Some(result)
}

pub(super) fn child(name: &str, attrs: Vec<(String, String)>, children: Vec<Node>) -> Node {
    Node::Element(Element {
        name: name.into(),
        attrs,
        children,
    })
}
pub(super) fn text_child(name: &str, value: String) -> Node {
    child(name, vec![], vec![Node::Text(value)])
}
pub(super) fn list_child(name: &str, kind: &str, values: Vec<String>) -> Node {
    child(
        name,
        vec![],
        vec![child(
            kind,
            vec![],
            values
                .into_iter()
                .map(|v| text_child("rdf:li", v))
                .collect(),
        )],
    )
}
pub(super) fn alt_child(name: &str, mut values: LangAltValue) -> Node {
    let default = values
        .remove("x-default")
        .map(|value| ("x-default".into(), value));
    child(
        name,
        vec![],
        vec![child(
            "rdf:Alt",
            vec![],
            default
                .into_iter()
                .chain(values)
                .map(|(lang, value)| {
                    child(
                        "rdf:li",
                        vec![("xml:lang".into(), lang)],
                        vec![Node::Text(value)],
                    )
                })
                .collect(),
        )],
    )
}
pub(super) fn remove_property(nodes: &mut [Node], parent: &Namespaces, uri: &str, local: &str) {
    for node in nodes {
        let Node::Element(e) = node else { continue };
        let ns = scope(parent, e);
        if is_element(e, &ns, RDF, "Description") {
            e.attrs
                .retain(|(name, _)| expanded(name, &ns, true) != Some((uri, local)));
            e.children.retain(|child| {
                if let Node::Element(prop) = child {
                    let prop_ns = scope(&ns, prop);
                    !is_element(prop, &prop_ns, uri, local)
                } else {
                    true
                }
            });
        } else {
            remove_property(&mut e.children, &ns, uri, local);
        }
    }
}
pub(super) fn first_description<'a>(
    nodes: &'a mut [Node],
    parent: &Namespaces,
) -> Option<&'a mut Element> {
    for node in nodes {
        let Node::Element(e) = node else { continue };
        let ns = scope(parent, e);
        if is_element(e, &ns, RDF, "Description") {
            return Some(e);
        }
        if let Some(found) = first_description(&mut e.children, &ns) {
            return Some(found);
        }
    }
    None
}

/// Parses one XMP packet into its preserved tree, reporting the preservation
/// failure that read-only callers tolerate and write callers must refuse.
pub(super) fn parse_document(
    bytes: &[u8],
    read_only: bool,
) -> Result<(Vec<Node>, Option<XmpParseError>), XmpParseError> {
    if bytes.len() as u64 > MAXIMUM_XMP_PACKET_BYTES {
        return Err(XmpParseError::ResourceLimit);
    }
    let packet = std::str::from_utf8(bytes).map_err(|_| XmpParseError::Malformed)?;
    if packet.chars().any(|c| {
        !matches!(c, '\t' | '\n' | '\r')
            && ((c as u32) < 0x20 || matches!(c, '\u{fffe}' | '\u{ffff}'))
    }) {
        return Err(XmpParseError::Malformed);
    }
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(false);
    let mut stack: Vec<Element> = Vec::new();
    let mut scopes = Vec::new();
    let mut roots = Vec::new();
    let mut buf = Vec::new();
    let mut nodes = 0;
    let mut item_count = 0;
    let mut lang_count = 0;
    let mut root_seen = false;
    let mut root_closed = false;
    let mut preservation_error = None;
    let version = XmlVersion::Implicit1_0;
    loop {
        buf.clear();
        match reader
            .read_event_into(&mut buf)
            .map_err(|_| XmpParseError::Malformed)?
        {
            event @ (Event::Start(_) | Event::Empty(_)) => {
                let empty = matches!(event, Event::Empty(_));
                let start = match event {
                    Event::Start(start) | Event::Empty(start) => start,
                    _ => return Err(XmpParseError::Malformed),
                };
                nodes += 1;
                if nodes > MAX_NODES || stack.len() + 1 > MAX_DEPTH {
                    return Err(XmpParseError::ResourceLimit);
                }
                if stack.is_empty() {
                    if root_seen {
                        return Err(XmpParseError::Malformed);
                    }
                    root_seen = true;
                }
                let e = parse_element(&start, version)?;
                let ns = scope(scopes.last().unwrap_or(&Namespaces::new()), &e);
                if !check_node(&e, &ns, &mut item_count, &mut lang_count)? {
                    if !read_only {
                        return Err(XmpParseError::Unpreservable);
                    }
                    preservation_error = Some(XmpParseError::Unpreservable);
                }
                if empty {
                    append(&mut stack, &mut roots, Node::Element(e));
                    if stack.is_empty() {
                        root_closed = true;
                    }
                } else {
                    scopes.push(ns);
                    stack.push(e);
                }
            }
            Event::End(end) => {
                let e = stack.pop().ok_or(XmpParseError::Malformed)?;
                scopes.pop();
                if end.name().as_ref() != e.name {
                    return Err(XmpParseError::Malformed);
                }
                let text_bytes: usize = e
                    .children
                    .iter()
                    .filter_map(|child| match child {
                        Node::Text(value) | Node::CData(value) => Some(value.len()),
                        _ => None,
                    })
                    .sum();
                if text_bytes > MAX_TEXT {
                    return Err(XmpParseError::ResourceLimit);
                }
                append(&mut stack, &mut roots, Node::Element(e));
                if stack.is_empty() {
                    root_closed = true;
                }
            }
            Event::Text(t) => {
                let decoded = unescape(t.xml_content(version).as_ref())
                    .map_err(|_| XmpParseError::Malformed)?
                    .into_owned();
                if decoded.len() > MAX_TEXT {
                    return Err(XmpParseError::ResourceLimit);
                }
                if stack.is_empty() {
                    if !decoded.trim().is_empty() {
                        return Err(XmpParseError::Malformed);
                    }
                } else {
                    append(&mut stack, &mut roots, Node::Text(decoded));
                }
            }
            Event::GeneralRef(r) => {
                let raw = r.as_ref();
                let reference = format!("&{raw};");
                let decoded = match unescape(&reference) {
                    Ok(decoded) => decoded,
                    Err(_) if read_only && preservation_error.is_some() && !stack.is_empty() => {
                        append(
                            &mut stack,
                            &mut roots,
                            Node::UnresolvedReference(raw.into()),
                        );
                        continue;
                    }
                    Err(_) => return Err(XmpParseError::Malformed),
                };
                if decoded.len() > MAX_TEXT {
                    return Err(XmpParseError::ResourceLimit);
                }
                if stack.is_empty() {
                    return Err(XmpParseError::Malformed);
                }
                append(&mut stack, &mut roots, Node::Text(decoded.into_owned()));
            }
            Event::CData(t) => {
                let value = t.xml_content(version).into_owned();
                if value.len() > MAX_TEXT {
                    return Err(XmpParseError::ResourceLimit);
                }
                if stack.is_empty() {
                    return Err(XmpParseError::Malformed);
                }
                append(&mut stack, &mut roots, Node::CData(value));
            }
            Event::Comment(c) => {
                let raw = c.as_ref();
                if raw.len() > MAX_TEXT {
                    return Err(XmpParseError::ResourceLimit);
                }
                append(&mut stack, &mut roots, Node::Comment(raw.into()));
            }
            Event::PI(pi) => {
                let raw = pi.as_ref();
                if raw.len() > MAX_TEXT {
                    return Err(XmpParseError::ResourceLimit);
                }
                if !raw.starts_with("xpacket ") {
                    append(&mut stack, &mut roots, Node::Pi(raw.into()));
                }
            }
            Event::Decl(decl) => {
                if root_seen {
                    return Err(XmpParseError::Malformed);
                }
                let v = decl.version().map_err(|_| XmpParseError::Malformed)?;
                if v.as_ref() != "1.0" {
                    return Err(XmpParseError::Unpreservable);
                }
                if decl.encoding().is_some() {
                    let raw = decl.as_ref();
                    if !raw.to_ascii_lowercase().contains("utf-8") {
                        return Err(XmpParseError::Unpreservable);
                    }
                }
            }
            Event::DocType(_) if read_only => {
                // Never load external subsets or expand declared entities.
                preservation_error = Some(XmpParseError::Unpreservable);
            }
            Event::DocType(_) => return Err(XmpParseError::Unpreservable),
            Event::Eof => break,
        }
    }
    if !stack.is_empty() || !root_seen || !root_closed {
        return Err(XmpParseError::Malformed);
    }
    if !roots.iter().any(|node| match node {
        Node::Element(e) => {
            let ns = scope(&Namespaces::new(), e);
            is_element(e, &ns, META, "xmpmeta") || is_element(e, &ns, RDF, "RDF")
        }
        _ => false,
    }) {
        return Err(XmpParseError::Malformed);
    }
    Ok((roots, preservation_error))
}

/// Wraps the preserved tree in the XMP packet header and trailer, redeclaring
/// the `xmpmeta` and `rdf:RDF` wrappers the input omitted.
pub(super) fn serialize_document(roots: &[Node]) -> String {
    let mut out = String::from("<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n");
    let wrapped = roots.iter().any(|node| match node {
        Node::Element(e) => {
            let ns = scope(&Namespaces::new(), e);
            is_element(e, &ns, META, "xmpmeta")
        }
        _ => false,
    });
    if !wrapped {
        out.push_str("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">");
    }
    let has_rdf = roots.iter().any(|node| match node {
        Node::Element(e) => {
            let ns = scope(&Namespaces::new(), e);
            is_element(e, &ns, RDF, "RDF")
        }
        _ => false,
    });
    if !wrapped && !has_rdf {
        out.push_str("<rdf:RDF xmlns:rdf=\"");
        out.push_str(RDF);
        out.push_str("\">");
    }
    for node in roots {
        serialize(node, &mut out);
    }
    if !wrapped && !has_rdf {
        out.push_str("</rdf:RDF>");
    }
    if !wrapped {
        out.push_str("</x:xmpmeta>");
    }
    out.push_str("<?xpacket end=\"w\"?>");
    out
}
