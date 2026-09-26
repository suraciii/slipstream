//! Bounded XMP parsing, typed fields, and checked sidecar patches.
use quick_xml::{
    XmlVersion,
    escape::unescape,
    events::{BytesStart, Event},
    reader::Reader,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

pub const MAXIMUM_XMP_PACKET_BYTES: u64 = 16 * 1024 * 1024;
const MAX_TEXT: usize = 64 * 1024;
const MAX_NODES: usize = 100_000;
const MAX_ITEMS: usize = 16_384;
const MAX_LANGUAGES: usize = 4096;
const MAX_DEPTH: usize = 128;
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
const DC: &str = "http://purl.org/dc/elements/1.1/";
const XMP: &str = "http://ns.adobe.com/xap/1.0/";
const RIGHTS: &str = "http://ns.adobe.com/xap/1.0/rights/";
const PHOTOSHOP: &str = "http://ns.adobe.com/photoshop/1.0/";
const EXIF: &str = "http://ns.adobe.com/exif/1.0/";
const TIFF: &str = "http://ns.adobe.com/tiff/1.0/";
const XML: &str = "http://www.w3.org/XML/1998/namespace";
const META: &str = "adobe:ns:meta/";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XmpParseError {
    Malformed,
    ResourceLimit,
    Unpreservable,
}
impl fmt::Display for XmpParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Malformed => "malformed XMP",
            Self::ResourceLimit => "XMP resource limit",
            Self::Unpreservable => "unpreservable XMP construct",
        })
    }
}
impl Error for XmpParseError {}

#[derive(Clone, Debug, PartialEq)]
pub enum FieldState<T> {
    Present(T),
    Absent,
    Invalid,
}
#[derive(Clone, Debug, PartialEq)]
pub enum PatchValue<T> {
    Set(T),
    /// Change and remove named language alternatives in one atomic field patch.
    SetLanguages {
        sets: LangAltValue,
        removes: Vec<String>,
    },
    Clear,
    Remove,
}
/// Language tags are stored in a deterministic order; input casing is canonicalized on save.
pub type LangAltValue = BTreeMap<String, String>;
/// A fractional XMP rating is never rounded to an integer.
pub type RatingValue = f64;
#[derive(Clone, Debug, PartialEq)]
pub enum FieldPatch {
    Title(PatchValue<LangAltValue>),
    Description(PatchValue<LangAltValue>),
    Headline(PatchValue<String>),
    Keywords(PatchValue<Vec<String>>),
    Label(PatchValue<String>),
    Rating(PatchValue<RatingValue>),
    Creators(PatchValue<Vec<String>>),
    CreatorsPosition(PatchValue<String>),
    Credit(PatchValue<String>),
    Source(PatchValue<String>),
    Rights(PatchValue<LangAltValue>),
    UsageTerms(PatchValue<LangAltValue>),
    Marked(PatchValue<bool>),
    WebStatement(PatchValue<String>),
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PatchRefusal {
    pub fields: Vec<String>,
    pub languages: BTreeMap<String, Vec<String>>,
}
impl fmt::Display for PatchRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "XMP patch refused for {}", self.fields.join(", "))
    }
}
impl Error for PatchRefusal {}

#[derive(Clone, Debug)]
enum Node {
    Element(Element),
    Text(String),
    CData(String),
    Comment(String),
    Pi(String),
}
#[derive(Clone, Debug)]
struct Element {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
}
type Namespaces = BTreeMap<String, String>;

fn scope(parent: &Namespaces, element: &Element) -> Namespaces {
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
fn expanded<'a>(name: &'a str, ns: &'a Namespaces, attribute: bool) -> Option<(&'a str, &'a str)> {
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
fn is_element(e: &Element, ns: &Namespaces, uri: &str, local: &str) -> bool {
    expanded(&e.name, ns, false) == Some((uri, local))
}
fn attribute<'a>(e: &'a Element, ns: &Namespaces, uri: &str, local: &str) -> Option<&'a str> {
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
) -> Result<(), XmpParseError> {
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
    for (name, value) in &node.attrs {
        if let Some((uri, local)) = expanded(name, ns, true) {
            if uri == RDF
                && (matches!(local, "ID" | "nodeID" | "bagID")
                    || local == "parseType" && value != "Resource"
                    || local == "about" && (value.starts_with('#') || value.contains("xpointer(")))
            {
                return Err(XmpParseError::Unpreservable);
            }
        } else if !name.starts_with("xmlns:") && name != "xmlns" {
            return Err(XmpParseError::Unpreservable);
        }
    }
    // Unbound names cannot be interpreted, but structurally malformed XML is
    // diagnosed by the event reader before preservation rules apply.
    if expanded(&node.name, ns, false).is_none() && node.name.contains(':') {
        return Err(XmpParseError::Unpreservable);
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
    Ok(())
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
fn text(e: &Element) -> Option<String> {
    let mut value = String::new();
    for child in &e.children {
        match child {
            Node::Text(s) | Node::CData(s) => value.push_str(s),
            _ => return None,
        }
    }
    Some(value)
}
fn single_container<'a>(
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
fn items<'a>(container: &'a Element, ns: &Namespaces) -> Option<Vec<(&'a Element, Namespaces)>> {
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
fn canonical_lang(lang: &str) -> Option<String> {
    if lang.eq_ignore_ascii_case("x-default") {
        return Some("x-default".into());
    }
    if lang.len() > 255 || !lang.is_ascii() {
        return None;
    }
    let mut parts = lang.split('-');
    let first = parts.next()?;
    if first.len() > 8
        || !(first.len() >= 2 || first.eq_ignore_ascii_case("x") || first.eq_ignore_ascii_case("i"))
        || !first.bytes().all(|b| b.is_ascii_alphabetic())
    {
        return None;
    }
    for part in parts {
        if part.is_empty() || part.len() > 8 || !part.bytes().all(|b| b.is_ascii_alphanumeric()) {
            return None;
        }
    }
    Some(lang.to_ascii_lowercase())
}
fn language_value(e: &Element, ns: &Namespaces) -> Option<LangAltValue> {
    let (container, container_ns) = single_container(e, ns, "Alt")?;
    let mut result = BTreeMap::new();
    for (li, li_ns) in items(container, &container_ns)? {
        let lang = canonical_lang(attribute(li, &li_ns, XML, "lang")?)?;
        let value = text(li)?;
        if result.insert(lang, value).is_some() {
            return None;
        }
    }
    Some(result)
}
fn list_value(e: &Element, ns: &Namespaces, kind: &str) -> Option<Vec<String>> {
    let (container, container_ns) = single_container(e, ns, kind)?;
    items(container, &container_ns)?
        .into_iter()
        .map(|(li, _)| text(li))
        .collect()
}
fn matching_properties<'a>(
    nodes: &'a [Node],
    parent: &Namespaces,
    uri: &str,
    local: &str,
    result: &mut Vec<Property<'a>>,
) {
    for node in nodes {
        let Node::Element(e) = node else { continue };
        let ns = scope(parent, e);
        if is_element(e, &ns, RDF, "Description") {
            for (key, value) in &e.attrs {
                if expanded(key, &ns, true) == Some((uri, local)) {
                    result.push(Property::Attribute(value));
                }
            }
            for child in &e.children {
                if let Node::Element(prop) = child {
                    let prop_ns = scope(&ns, prop);
                    if is_element(prop, &prop_ns, uri, local) {
                        result.push(Property::Element(prop, prop_ns));
                    }
                }
            }
        } else {
            matching_properties(&e.children, &ns, uri, local, result);
        }
    }
}
enum Property<'a> {
    Attribute(&'a str),
    Element(&'a Element, Namespaces),
}
fn property_text(prop: &Property<'_>) -> Option<String> {
    match prop {
        Property::Attribute(v) => Some((*v).into()),
        Property::Element(e, ns) => {
            if e.attrs.iter().any(|(name, _)| {
                expanded(name, ns, true) == Some((RDF, "resource"))
                    || expanded(name, ns, true) == Some((RDF, "parseType"))
            }) {
                return None;
            }
            text(e)
        }
    }
}
fn name_parts(name: &str) -> (&str, &str) {
    let (prefix, local) = name.split_once(':').unwrap_or(("", name));
    (
        match prefix {
            "dc" => DC,
            "xmp" => XMP,
            "xmpRights" => RIGHTS,
            "photoshop" => PHOTOSHOP,
            "exif" => EXIF,
            "tiff" => TIFF,
            _ => "",
        },
        local,
    )
}
fn child(name: &str, attrs: Vec<(String, String)>, children: Vec<Node>) -> Node {
    Node::Element(Element {
        name: name.into(),
        attrs,
        children,
    })
}
fn text_child(name: &str, value: String) -> Node {
    child(name, vec![], vec![Node::Text(value)])
}
fn list_child(name: &str, kind: &str, values: Vec<String>) -> Node {
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
fn alt_child(name: &str, mut values: LangAltValue) -> Node {
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
fn remove_property(nodes: &mut [Node], parent: &Namespaces, uri: &str, local: &str) {
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
fn first_description<'a>(nodes: &'a mut [Node], parent: &Namespaces) -> Option<&'a mut Element> {
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

/// Parsed XML tree, preserving unknown RDF structures, namespace bindings, and attributes.
#[derive(Clone, Debug)]
pub struct XmpDocument {
    roots: Vec<Node>,
}
impl XmpDocument {
    pub fn parse(bytes: &[u8]) -> Result<Self, XmpParseError> {
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
                    check_node(&e, &ns, &mut item_count, &mut lang_count)?;
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
                    let decoded = unescape(&reference).map_err(|_| XmpParseError::Malformed)?;
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
                Event::DocType(_) => return Err(XmpParseError::Unpreservable),
                Event::Eof => break,
            }
        }
        if !stack.is_empty() || !root_seen || !root_closed {
            return Err(XmpParseError::Malformed);
        }
        let doc = Self { roots };
        if !doc.roots.iter().any(|node| match node {
            Node::Element(e) => {
                let ns = scope(&Namespaces::new(), e);
                is_element(e, &ns, META, "xmpmeta") || is_element(e, &ns, RDF, "RDF")
            }
            _ => false,
        }) {
            return Err(XmpParseError::Malformed);
        }
        Ok(doc)
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out =
            String::from("<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n");
        let wrapped = self.roots.iter().any(|node| match node {
            Node::Element(e) => {
                let ns = scope(&Namespaces::new(), e);
                is_element(e, &ns, META, "xmpmeta")
            }
            _ => false,
        });
        if !wrapped {
            out.push_str("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">");
        }
        let has_rdf = self.roots.iter().any(|node| match node {
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
        for node in &self.roots {
            serialize(node, &mut out);
        }
        if !wrapped && !has_rdf {
            out.push_str("</rdf:RDF>");
        }
        if !wrapped {
            out.push_str("</x:xmpmeta>");
        }
        out.push_str("<?xpacket end=\"w\"?>");
        out.into_bytes()
    }
    fn properties(&self, name: &str) -> Vec<Property<'_>> {
        let (uri, local) = name_parts(name);
        let mut result = Vec::new();
        matching_properties(&self.roots, &Namespaces::new(), uri, local, &mut result);
        result
    }
    fn simple(&self, name: &str) -> FieldState<String> {
        let properties = self.properties(name);
        match properties.as_slice() {
            [] => FieldState::Absent,
            [one] => property_text(one).map_or(FieldState::Invalid, FieldState::Present),
            _ => FieldState::Invalid,
        }
    }
    fn lang(&self, name: &str) -> FieldState<LangAltValue> {
        let properties = self.properties(name);
        match properties.as_slice() {
            [] => FieldState::Absent,
            [Property::Element(e, ns)] => {
                language_value(e, ns).map_or(FieldState::Invalid, FieldState::Present)
            }
            _ => FieldState::Invalid,
        }
    }
    fn list(&self, name: &str, kind: &str) -> FieldState<Vec<String>> {
        let properties = self.properties(name);
        match properties.as_slice() {
            [] => FieldState::Absent,
            [Property::Element(e, ns)] => {
                list_value(e, ns, kind).map_or(FieldState::Invalid, FieldState::Present)
            }
            _ => FieldState::Invalid,
        }
    }
    pub fn title(&self) -> FieldState<LangAltValue> {
        self.lang("dc:title")
    }
    pub fn description(&self) -> FieldState<LangAltValue> {
        self.lang("dc:description")
    }
    pub fn rights(&self) -> FieldState<LangAltValue> {
        self.lang("dc:rights")
    }
    pub fn usage_terms(&self) -> FieldState<LangAltValue> {
        self.lang("xmpRights:UsageTerms")
    }
    pub fn headline(&self) -> FieldState<String> {
        self.simple("photoshop:Headline")
    }
    pub fn label(&self) -> FieldState<String> {
        self.simple("xmp:Label")
    }
    pub fn web_statement(&self) -> FieldState<String> {
        self.simple("xmpRights:WebStatement")
    }
    pub fn creators_position(&self) -> FieldState<String> {
        self.simple("photoshop:AuthorsPosition")
    }
    pub fn credit(&self) -> FieldState<String> {
        self.simple("photoshop:Credit")
    }
    pub fn source(&self) -> FieldState<String> {
        self.simple("photoshop:Source")
    }
    pub fn keywords(&self) -> FieldState<Vec<String>> {
        match self.list("dc:subject", "Bag") {
            FieldState::Present(values) => {
                let mut seen = BTreeSet::new();
                FieldState::Present(
                    values
                        .into_iter()
                        .filter(|v| seen.insert(v.clone()))
                        .collect(),
                )
            }
            other => other,
        }
    }
    pub fn creators(&self) -> FieldState<Vec<String>> {
        self.list("dc:creator", "Seq")
    }
    pub fn rating(&self) -> FieldState<RatingValue> {
        match self.simple("xmp:Rating") {
            FieldState::Present(v) => match v.parse::<f64>() {
                Ok(n) if valid_rating(n) => FieldState::Present(n),
                _ => FieldState::Invalid,
            },
            FieldState::Absent => FieldState::Absent,
            FieldState::Invalid => FieldState::Invalid,
        }
    }
    pub fn marked(&self) -> FieldState<bool> {
        match self.simple("xmpRights:Marked") {
            FieldState::Present(v) => match v.as_str() {
                "True" => FieldState::Present(true),
                "False" => FieldState::Present(false),
                _ => FieldState::Invalid,
            },
            FieldState::Absent => FieldState::Absent,
            FieldState::Invalid => FieldState::Invalid,
        }
    }
    pub fn capture_representations(&self) -> Vec<(&'static str, String)> {
        [
            "exif:DateTimeOriginal",
            "exif:PixelXDimension",
            "exif:PixelYDimension",
            "tiff:Orientation",
            "exif:LensModel",
            "tiff:Make",
            "tiff:Model",
        ]
        .into_iter()
        .filter_map(|name| match self.simple(name) {
            FieldState::Present(value) => Some((name, value)),
            _ => None,
        })
        .collect()
    }
    /// Validate the entire request first; only a fully valid request mutates this document.
    /// Patches are checked against the pre-patch document. Language `Set` merges
    /// named alternatives; `SetLanguages` also removes named alternatives.
    /// `Clear` writes one empty x-default alternative, while `Remove` deletes
    /// the complete property. Numeric and Boolean fields cannot be cleared.
    pub fn apply(&mut self, patches: &[FieldPatch]) -> Result<(), PatchRefusal> {
        let mut refusal = PatchRefusal::default();
        let mut seen = BTreeSet::new();
        let mut edits = Vec::new();
        for patch in patches {
            let (name, candidate) = self.prepare(patch, &mut refusal);
            if !seen.insert(name) {
                refuse(&mut refusal, name);
            }
            edits.push((name, candidate));
        }
        if !refusal.fields.is_empty() {
            return Err(refusal);
        }
        for (name, candidate) in edits {
            let (uri, local) = name_parts(name);
            remove_property(&mut self.roots, &Namespaces::new(), uri, local);
            if let Some(mut node) = candidate {
                if let Some(desc) = first_description(&mut self.roots, &Namespaces::new()) {
                    let ns = scope(&Namespaces::new(), desc);
                    let (prefix, _) = name.split_once(':').unwrap_or(("", name));
                    if let Node::Element(prop) = &mut node {
                        let prop_ns = scope(&ns, prop);
                        for (prefix, uri) in [(prefix, uri), ("rdf", RDF)] {
                            if prop_ns.get(prefix).map(String::as_str) != Some(uri)
                                && (prefix == "rdf"
                                    && !prop.attrs.iter().any(|(key, _)| key == "xmlns:rdf")
                                    || prop.name.starts_with(&format!("{prefix}:")))
                            {
                                prop.attrs.push((format!("xmlns:{prefix}"), uri.into()));
                            }
                        }
                    }
                    desc.children.push(node);
                } else {
                    let desc = Element {
                        name: "rdf:Description".into(),
                        attrs: vec![
                            ("xmlns:rdf".into(), RDF.into()),
                            (
                                format!("xmlns:{}", name.split_once(':').map_or("dc", |p| p.0)),
                                uri.into(),
                            ),
                        ],
                        children: vec![node],
                    };
                    let mut candidate = Some(desc);
                    for root in &mut self.roots {
                        if let Node::Element(e) = root {
                            let ns = scope(&Namespaces::new(), e);
                            if is_element(e, &ns, RDF, "RDF") {
                                if let Some(desc) = candidate.take() {
                                    e.children.push(Node::Element(desc));
                                }
                                break;
                            }
                            if is_element(e, &ns, META, "xmpmeta") {
                                if let Some(desc) = candidate.take() {
                                    e.children.push(child(
                                        "rdf:RDF",
                                        vec![("xmlns:rdf".into(), RDF.into())],
                                        vec![Node::Element(desc)],
                                    ));
                                }
                                break;
                            }
                        }
                    }
                    if let Some(desc) = candidate {
                        self.roots.push(child(
                            "rdf:RDF",
                            vec![("xmlns:rdf".into(), RDF.into())],
                            vec![Node::Element(desc)],
                        ));
                    }
                }
            }
        }
        Ok(())
    }
    fn prepare(
        &self,
        patch: &FieldPatch,
        refusal: &mut PatchRefusal,
    ) -> (&'static str, Option<Node>) {
        macro_rules! simple {
            ($name:expr, $p:expr) => {{
                let name = $name;
                (
                    name,
                    match $p {
                        PatchValue::Set(v) => {
                            checked_text(name, v, refusal).then(|| text_child(name, v.clone()))
                        }
                        PatchValue::Clear => Some(text_child(name, String::new())),
                        PatchValue::SetLanguages { .. } => {
                            refuse(refusal, name);
                            None
                        }
                        PatchValue::Remove => None,
                    },
                )
            }};
        }
        macro_rules! lang {
            ($name:expr, $p:expr) => {{
                let name = $name;
                (name, self.prepare_lang(name, $p, refusal))
            }};
        }
        match patch {
            FieldPatch::Title(v) => lang!("dc:title", v),
            FieldPatch::Description(v) => lang!("dc:description", v),
            FieldPatch::Rights(v) => lang!("dc:rights", v),
            FieldPatch::UsageTerms(v) => lang!("xmpRights:UsageTerms", v),
            FieldPatch::Headline(v) => simple!("photoshop:Headline", v),
            FieldPatch::Label(v) => simple!("xmp:Label", v),
            FieldPatch::WebStatement(v) => simple!("xmpRights:WebStatement", v),
            FieldPatch::CreatorsPosition(v) => simple!("photoshop:AuthorsPosition", v),
            FieldPatch::Credit(v) => simple!("photoshop:Credit", v),
            FieldPatch::Source(v) => simple!("photoshop:Source", v),
            FieldPatch::Keywords(v) => (
                "dc:subject",
                match v {
                    PatchValue::Set(list) => {
                        if list.len() > MAX_ITEMS {
                            refuse(refusal, "dc:subject");
                        }
                        let mut seen = BTreeSet::new();
                        for text in list {
                            checked_text("dc:subject", text, refusal);
                        }
                        Some(list_child(
                            "dc:subject",
                            "rdf:Bag",
                            list.iter()
                                .filter(|v| seen.insert((*v).clone()))
                                .cloned()
                                .collect(),
                        ))
                    }
                    PatchValue::Clear => Some(list_child("dc:subject", "rdf:Bag", vec![])),
                    PatchValue::Remove => None,
                    PatchValue::SetLanguages { .. } => {
                        refuse(refusal, "dc:subject");
                        None
                    }
                },
            ),
            FieldPatch::Creators(v) => (
                "dc:creator",
                match v {
                    PatchValue::Set(list) => {
                        if list.len() > MAX_ITEMS {
                            refuse(refusal, "dc:creator");
                        }
                        for text in list {
                            checked_text("dc:creator", text, refusal);
                        }
                        Some(list_child("dc:creator", "rdf:Seq", list.clone()))
                    }
                    PatchValue::Clear => Some(list_child("dc:creator", "rdf:Seq", vec![])),
                    PatchValue::Remove => None,
                    PatchValue::SetLanguages { .. } => {
                        refuse(refusal, "dc:creator");
                        None
                    }
                },
            ),
            FieldPatch::Rating(v) => (
                "xmp:Rating",
                match v {
                    PatchValue::Set(value) => {
                        if !valid_rating(*value) {
                            refuse(refusal, "xmp:Rating");
                            None
                        } else {
                            Some(text_child("xmp:Rating", value.to_string()))
                        }
                    }
                    PatchValue::Clear => {
                        refuse(refusal, "xmp:Rating");
                        None
                    }
                    PatchValue::Remove => None,
                    PatchValue::SetLanguages { .. } => {
                        refuse(refusal, "xmp:Rating");
                        None
                    }
                },
            ),
            FieldPatch::Marked(v) => (
                "xmpRights:Marked",
                match v {
                    PatchValue::Set(true) => Some(text_child("xmpRights:Marked", "True".into())),
                    PatchValue::Set(false) => Some(text_child("xmpRights:Marked", "False".into())),
                    PatchValue::Clear => {
                        refuse(refusal, "xmpRights:Marked");
                        None
                    }
                    PatchValue::Remove => None,
                    PatchValue::SetLanguages { .. } => {
                        refuse(refusal, "xmpRights:Marked");
                        None
                    }
                },
            ),
        }
    }
    fn prepare_lang(
        &self,
        name: &'static str,
        patch: &PatchValue<LangAltValue>,
        refusal: &mut PatchRefusal,
    ) -> Option<Node> {
        if matches!(patch, PatchValue::Remove) {
            return None;
        }
        let mut existing = match self.lang(name) {
            FieldState::Present(values) => values,
            FieldState::Absent => BTreeMap::new(),
            FieldState::Invalid => {
                refuse(refusal, name);
                return None;
            }
        };
        let (changes, removals) = match patch {
            PatchValue::Set(values) => (values.clone(), Vec::new()),
            PatchValue::SetLanguages { sets, removes } => (sets.clone(), removes.clone()),
            PatchValue::Clear => {
                existing.clear();
                (
                    BTreeMap::from([("x-default".into(), String::new())]),
                    Vec::new(),
                )
            }
            PatchValue::Remove => return None,
        };
        let mut named = BTreeSet::new();
        let mut pending = Vec::new();
        for (tag, value) in changes {
            let Some(canonical) = canonical_lang(&tag) else {
                refuse(refusal, name);
                continue;
            };
            if !named.insert(canonical.clone()) {
                refuse(refusal, name);
            }
            if !checked_text(name, &value, refusal) {
                continue;
            }
            pending.push((canonical, value));
        }
        let mut remove = Vec::new();
        for tag in removals {
            let Some(canonical) = canonical_lang(&tag) else {
                refuse(refusal, name);
                continue;
            };
            if !named.insert(canonical.clone()) {
                refuse(refusal, name);
            }
            remove.push(canonical);
        }
        // A mirrored default must be named when its source is changed or removed.
        if let Some(default) = existing.get("x-default") {
            let mirror_changed = pending.iter().any(|(tag, replacement)| {
                tag != "x-default" && existing.get(tag) == Some(default) && replacement != default
            }) || remove
                .iter()
                .any(|tag| tag != "x-default" && existing.get(tag) == Some(default));
            if mirror_changed && !named.contains("x-default") {
                refuse(refusal, name);
                refusal
                    .languages
                    .entry(name.into())
                    .or_default()
                    .push("x-default".into());
            }
        }
        let changed = named;
        for tag in remove {
            existing.remove(&tag);
        }
        for (tag, value) in pending {
            existing.insert(tag, value);
        }
        if existing.len() > MAX_LANGUAGES {
            refuse(refusal, name);
        }
        if existing.is_empty() {
            if matches!(patch, PatchValue::Set(_)) {
                Some(alt_child(name, existing))
            } else {
                None
            }
        } else {
            if !matches!(patch, PatchValue::Clear)
                && let [Property::Element(original, ns)] = self.properties(name).as_slice()
            {
                let mut property = (*original).clone();
                for (prefix, uri) in ns {
                    let key = if prefix.is_empty() {
                        "xmlns".into()
                    } else {
                        format!("xmlns:{prefix}")
                    };
                    if !property.attrs.iter().any(|(name, _)| name == &key) {
                        property.attrs.push((key, uri.clone()));
                    }
                }
                if let Some((_, container_ns)) = single_container(original, ns, "Alt")
                    && let Some(Node::Element(container)) = property
                        .children
                        .iter_mut()
                        .find(|node| matches!(node, Node::Element(_)))
                {
                    container.children.retain(|node| {
                        let Node::Element(li) = node else { return true };
                        let li_ns = scope(&container_ns, li);
                        let tag = attribute(li, &li_ns, XML, "lang").and_then(canonical_lang);
                        !tag.as_ref().is_some_and(|tag| changed.contains(tag))
                    });
                    for tag in &changed {
                        if let Some(value) = existing.get(tag) {
                            let item = child(
                                "rdf:li",
                                vec![
                                    ("xmlns:rdf".into(), RDF.into()),
                                    ("xml:lang".into(), tag.clone()),
                                ],
                                vec![Node::Text(value.clone())],
                            );
                            if tag == "x-default" {
                                container.children.insert(0, item);
                            } else {
                                container.children.push(item);
                            }
                        }
                    }
                    return Some(Node::Element(property));
                }
            }
            Some(alt_child(name, existing))
        }
    }
}
fn valid_rating(value: f64) -> bool {
    value.is_finite() && (value == -1.0 || (0.0..=5.0).contains(&value))
}
fn checked_text(name: &str, value: &str, refusal: &mut PatchRefusal) -> bool {
    if value.len() > MAX_TEXT
        || value.chars().any(|c| {
            !matches!(c, '\t' | '\n' | '\r')
                && ((c as u32) < 0x20 || matches!(c, '\u{fffe}' | '\u{ffff}'))
        })
    {
        refuse(refusal, name);
        false
    } else {
        true
    }
}
fn refuse(refusal: &mut PatchRefusal, name: &str) {
    if !refusal.fields.iter().any(|v| v == name) {
        refusal.fields.push(name.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCUMENT: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" xmlns:exif="http://ns.adobe.com/exif/1.0/"><rdf:RDF><rdf:Description rdf:about="" xmp:Label="Review" xmlns:tiff="http://ns.adobe.com/tiff/1.0/"><dc:title><rdf:Alt><rdf:li xml:lang="x-default">Hello</rdf:li><rdf:li xml:lang="fr-FR">Bonjour</rdf:li></rdf:Alt></dc:title><dc:subject><rdf:Bag><rdf:li>A</rdf:li><rdf:li>B</rdf:li></rdf:Bag></dc:subject><dc:creator><rdf:Seq><rdf:li>First</rdf:li><rdf:li>Second</rdf:li></rdf:Seq></dc:creator><crs:Settings rdf:parseType="Resource"><crs:Temperature>5900</crs:Temperature></crs:Settings><exif:DateTimeOriginal>2020:01:02 03:04:05</exif:DateTimeOriginal><tiff:Make>Camera</tiff:Make></rdf:Description><rdf:Description rdf:about="urn:photo:second" xmlns:custom="urn:custom"><custom:feature rdf:resource="https://example.invalid/a&amp;b" custom:keep="yes"/><custom:structure><rdf:Alt><rdf:li xml:lang="de-DE">Wert</rdf:li></rdf:Alt></custom:structure></rdf:Description></rdf:RDF></x:xmpmeta>"#;
    fn doc() -> XmpDocument {
        XmpDocument::parse(DOCUMENT.as_bytes()).unwrap()
    }
    fn minimal() -> XmpDocument {
        XmpDocument::parse(format!("<rdf:RDF xmlns:rdf=\"{RDF}\"/>").as_bytes()).unwrap()
    }
    fn reread(doc: &XmpDocument) -> XmpDocument {
        XmpDocument::parse(&doc.to_bytes()).unwrap()
    }
    fn lang(values: &[(&str, &str)]) -> LangAltValue {
        values
            .iter()
            .map(|(k, v)| ((*k).into(), (*v).into()))
            .collect()
    }

    #[test]
    fn adobe_structures_and_namespace_redeclarations_survive_patch() {
        let mut d = doc();
        assert_eq!(
            d.title(),
            FieldState::Present(lang(&[("x-default", "Hello"), ("fr-fr", "Bonjour")]))
        );
        assert_eq!(d.label(), FieldState::Present("Review".into()));
        assert_eq!(
            d.creators(),
            FieldState::Present(vec!["First".into(), "Second".into()])
        );
        d.apply(&[FieldPatch::Headline(PatchValue::Set("News".into()))])
            .unwrap();
        let xml = String::from_utf8(d.to_bytes()).unwrap();
        for fragment in [
            "rdf:about=\"urn:photo:second\"",
            "rdf:parseType=\"Resource\"",
            "crs:Temperature>5900",
            "rdf:resource=\"https://example.invalid/a&amp;b\"",
            "custom:keep=\"yes\"",
            "xml:lang=\"de-DE\"",
            "xmlns:tiff=",
        ] {
            assert!(xml.contains(fragment), "missing {fragment}");
        }
        let rt = reread(&d);
        assert_eq!(rt.headline(), FieldState::Present("News".into()));
        assert_eq!(rt.title(), d.title());
        assert_eq!(
            rt.keywords(),
            FieldState::Present(vec!["A".into(), "B".into()])
        );
        assert_eq!(rt.label(), d.label());
    }

    #[test]
    fn set_clear_remove_every_writable_field() {
        let mut d = minimal();
        let set = vec![
            FieldPatch::Title(PatchValue::Set(lang(&[("en-US", "Title")]))),
            FieldPatch::Description(PatchValue::Set(lang(&[("fr", "Description")]))),
            FieldPatch::Headline(PatchValue::Set("Headline".into())),
            FieldPatch::Keywords(PatchValue::Set(vec!["word".into()])),
            FieldPatch::Label(PatchValue::Set("Label".into())),
            FieldPatch::Rating(PatchValue::Set(3.5)),
            FieldPatch::Creators(PatchValue::Set(vec!["Ada".into(), "Bob".into()])),
            FieldPatch::CreatorsPosition(PatchValue::Set("Editor".into())),
            FieldPatch::Credit(PatchValue::Set("Credit".into())),
            FieldPatch::Source(PatchValue::Set("Source".into())),
            FieldPatch::Rights(PatchValue::Set(lang(&[("x-default", "Copyright")]))),
            FieldPatch::UsageTerms(PatchValue::Set(lang(&[("de-DE", "Terms")]))),
            FieldPatch::Marked(PatchValue::Set(false)),
            FieldPatch::WebStatement(PatchValue::Set("https://example.invalid/rights".into())),
        ];
        d.apply(&set).unwrap();
        let d = reread(&d);
        assert_eq!(d.title(), FieldState::Present(lang(&[("en-us", "Title")])));
        assert_eq!(
            d.description(),
            FieldState::Present(lang(&[("fr", "Description")]))
        );
        assert_eq!(d.headline(), FieldState::Present("Headline".into()));
        assert_eq!(d.keywords(), FieldState::Present(vec!["word".into()]));
        assert_eq!(d.label(), FieldState::Present("Label".into()));
        assert_eq!(d.rating(), FieldState::Present(3.5));
        assert_eq!(
            d.creators(),
            FieldState::Present(vec!["Ada".into(), "Bob".into()])
        );
        assert_eq!(d.creators_position(), FieldState::Present("Editor".into()));
        assert_eq!(d.credit(), FieldState::Present("Credit".into()));
        assert_eq!(d.source(), FieldState::Present("Source".into()));
        assert_eq!(
            d.rights(),
            FieldState::Present(lang(&[("x-default", "Copyright")]))
        );
        assert_eq!(
            d.usage_terms(),
            FieldState::Present(lang(&[("de-de", "Terms")]))
        );
        assert_eq!(d.marked(), FieldState::Present(false));
        assert_eq!(
            d.web_statement(),
            FieldState::Present("https://example.invalid/rights".into())
        );
        let mut d = d;
        d.apply(&[
            FieldPatch::Title(PatchValue::Clear),
            FieldPatch::Description(PatchValue::Clear),
            FieldPatch::Headline(PatchValue::Clear),
            FieldPatch::Keywords(PatchValue::Clear),
            FieldPatch::Label(PatchValue::Clear),
            FieldPatch::Rating(PatchValue::Set(0.0)),
            FieldPatch::Creators(PatchValue::Clear),
            FieldPatch::CreatorsPosition(PatchValue::Clear),
            FieldPatch::Credit(PatchValue::Clear),
            FieldPatch::Source(PatchValue::Clear),
            FieldPatch::Rights(PatchValue::Clear),
            FieldPatch::UsageTerms(PatchValue::Clear),
            FieldPatch::Marked(PatchValue::Set(false)),
            FieldPatch::WebStatement(PatchValue::Clear),
        ])
        .unwrap();
        let d = reread(&d);
        assert_eq!(d.title(), FieldState::Present(lang(&[("x-default", "")])));
        assert_eq!(
            d.description(),
            FieldState::Present(lang(&[("x-default", "")]))
        );
        for field in [
            d.headline(),
            d.label(),
            d.creators_position(),
            d.credit(),
            d.source(),
            d.web_statement(),
        ] {
            assert_eq!(field, FieldState::Present(String::new()));
        }
        assert_eq!(d.keywords(), FieldState::Present(vec![]));
        assert_eq!(d.creators(), FieldState::Present(vec![]));
        assert_eq!(d.rating(), FieldState::Present(0.0));
        assert_eq!(d.marked(), FieldState::Present(false));
        assert_eq!(d.rights(), FieldState::Present(lang(&[("x-default", "")])));
        assert_eq!(
            d.usage_terms(),
            FieldState::Present(lang(&[("x-default", "")]))
        );
        let mut d = d;
        d.apply(&[
            FieldPatch::Title(PatchValue::Remove),
            FieldPatch::Description(PatchValue::Remove),
            FieldPatch::Headline(PatchValue::Remove),
            FieldPatch::Keywords(PatchValue::Remove),
            FieldPatch::Label(PatchValue::Remove),
            FieldPatch::Rating(PatchValue::Remove),
            FieldPatch::Creators(PatchValue::Remove),
            FieldPatch::CreatorsPosition(PatchValue::Remove),
            FieldPatch::Credit(PatchValue::Remove),
            FieldPatch::Source(PatchValue::Remove),
            FieldPatch::Rights(PatchValue::Remove),
            FieldPatch::UsageTerms(PatchValue::Remove),
            FieldPatch::Marked(PatchValue::Remove),
            FieldPatch::WebStatement(PatchValue::Remove),
        ])
        .unwrap();
        let d = reread(&d);
        assert_eq!(d.title(), FieldState::Absent);
        assert_eq!(d.description(), FieldState::Absent);
        for field in [
            d.headline(),
            d.label(),
            d.creators_position(),
            d.credit(),
            d.source(),
            d.web_statement(),
        ] {
            assert_eq!(field, FieldState::Absent);
        }
        assert_eq!(d.keywords(), FieldState::Absent);
        assert_eq!(d.creators(), FieldState::Absent);
        assert_eq!(d.rating(), FieldState::Absent);
        assert_eq!(d.marked(), FieldState::Absent);
        assert_eq!(d.rights(), FieldState::Absent);
        assert_eq!(d.usage_terms(), FieldState::Absent);
    }

    #[test]
    fn rating_boundaries_invalid_values_and_atomicity() {
        let mut d = minimal();
        for n in [-1.0, 0.0, 3.5, 5.0] {
            d.apply(&[FieldPatch::Rating(PatchValue::Set(n))]).unwrap();
            assert_eq!(reread(&d).rating(), FieldState::Present(n));
        }
        let before = d.to_bytes();
        let bad = d
            .apply(&[
                FieldPatch::Headline(PatchValue::Set("new".into())),
                FieldPatch::Rating(PatchValue::Set(5.1)),
                FieldPatch::Marked(PatchValue::Clear),
            ])
            .unwrap_err();
        assert_eq!(bad.fields, vec!["xmp:Rating", "xmpRights:Marked"]);
        assert_eq!(d.to_bytes(), before);
        for value in [f64::NAN, f64::INFINITY, -0.1, -2.0] {
            assert!(
                d.apply(&[FieldPatch::Rating(PatchValue::Set(value))])
                    .is_err()
            );
        }
        assert_eq!(d.to_bytes(), before);
    }

    #[test]
    fn keyword_duplicate_case_and_creator_order() {
        let mut d = minimal();
        d.apply(&[
            FieldPatch::Keywords(PatchValue::Set(vec![
                "Leaf".into(),
                "leaf".into(),
                "Leaf".into(),
            ])),
            FieldPatch::Creators(PatchValue::Set(vec!["Z".into(), "A".into(), "Z".into()])),
        ])
        .unwrap();
        let d = reread(&d);
        assert_eq!(
            d.keywords(),
            FieldState::Present(vec!["Leaf".into(), "leaf".into()])
        );
        assert_eq!(
            d.creators(),
            FieldState::Present(vec!["Z".into(), "A".into(), "Z".into()])
        );
    }

    #[test]
    fn alternative_patch_preserves_others_and_requires_named_default() {
        let mut d = doc();
        let before = d.to_bytes();
        let refusal = d
            .apply(&[
                FieldPatch::Title(PatchValue::Set(lang(&[
                    ("X-DEFAULT", "Hello"),
                    ("FR-fr", "Salut"),
                ]))),
                FieldPatch::Description(PatchValue::Set(lang(&[("de", "Beschreibung")]))),
            ])
            .unwrap();
        assert_eq!(refusal, ());
        assert_eq!(
            d.title(),
            FieldState::Present(lang(&[("x-default", "Hello"), ("fr-fr", "Salut")]))
        );
        assert_ne!(d.to_bytes(), before);
        let mut mirrored = minimal();
        mirrored
            .apply(&[FieldPatch::Title(PatchValue::Set(lang(&[
                ("x-default", "Same"),
                ("en-US", "Same"),
                ("de", "Other"),
            ])))])
            .unwrap();
        let before = mirrored.to_bytes();
        let refusal = mirrored
            .apply(&[
                FieldPatch::Title(PatchValue::Set(lang(&[("EN-us", "Changed")]))),
                FieldPatch::Headline(PatchValue::Set("never".into())),
            ])
            .unwrap_err();
        assert_eq!(refusal.fields, vec!["dc:title"]);
        assert_eq!(refusal.languages["dc:title"], vec!["x-default"]);
        assert_eq!(mirrored.to_bytes(), before);
        mirrored
            .apply(&[FieldPatch::Title(PatchValue::Set(lang(&[
                ("en-us", "Changed"),
                ("x-default", "Changed"),
            ])))])
            .unwrap();
        assert_eq!(
            mirrored.title(),
            FieldState::Present(lang(&[
                ("de", "Other"),
                ("en-us", "Changed"),
                ("x-default", "Changed")
            ]))
        );
    }

    #[test]
    fn removes_one_language_and_retains_empty_values() {
        let mut d = doc();
        d.apply(&[FieldPatch::Title(PatchValue::SetLanguages {
            sets: lang(&[("en-US", "")]),
            removes: vec!["FR-fr".into()],
        })])
        .unwrap();
        assert_eq!(
            reread(&d).title(),
            FieldState::Present(lang(&[("en-us", ""), ("x-default", "Hello")]))
        );
        let before = d.to_bytes();
        for removes in [vec!["EN-us".into(), "en-US".into()], vec!["en-us".into()]] {
            assert!(
                d.apply(&[FieldPatch::Title(PatchValue::SetLanguages {
                    sets: lang(&[("en-us", "new")]),
                    removes
                })])
                .is_err()
            );
            assert_eq!(d.to_bytes(), before);
        }
        d.apply(&[FieldPatch::Title(PatchValue::Set(lang(&[(
            "en-us", "Hello",
        )])))])
        .unwrap();
        let before = d.to_bytes();
        let refusal = d
            .apply(&[FieldPatch::Title(PatchValue::SetLanguages {
                sets: lang(&[]),
                removes: vec!["en-us".into()],
            })])
            .unwrap_err();
        assert_eq!(refusal.languages["dc:title"], vec!["x-default"]);
        assert_eq!(d.to_bytes(), before);
        d.apply(&[FieldPatch::Title(PatchValue::SetLanguages {
            sets: lang(&[]),
            removes: vec!["en-us".into(), "x-default".into()],
        })])
        .unwrap();
        assert_eq!(reread(&d).title(), FieldState::Absent);
    }

    #[test]
    fn namespace_aliases_and_untouched_language_qualifiers_survive() {
        let xml = format!(
            "<r:RDF xmlns:r=\"{RDF}\" xmlns:d=\"{DC}\" xmlns:rdf=\"urn:foreign\" xmlns:q=\"urn:qualifier\"><r:Description><d:title q:property=\"keep\"><r:Alt q:container=\"keep\"><r:li xml:lang=\"fr-FR\" q:quality=\"human\">Bonjour</r:li><r:li xml:lang=\"en-US\">Hi</r:li></r:Alt></d:title><rdf:foreign>untouched</rdf:foreign></r:Description></r:RDF>"
        );
        let mut d = XmpDocument::parse(xml.as_bytes()).unwrap();
        d.apply(&[FieldPatch::Title(PatchValue::Set(lang(&[(
            "en-us", "Hello",
        )])))])
        .unwrap();
        let bytes = d.to_bytes();
        let out = String::from_utf8(bytes.clone()).unwrap();
        assert!(out.contains("xml:lang=\"fr-FR\" q:quality=\"human\""));
        assert!(out.contains("q:property=\"keep\""));
        assert!(out.contains("q:container=\"keep\""));
        assert!(out.contains("<rdf:foreign>untouched</rdf:foreign>"));
        let rt = XmpDocument::parse(&bytes).unwrap();
        assert_eq!(
            rt.title(),
            FieldState::Present(lang(&[("fr-fr", "Bonjour"), ("en-us", "Hello")]))
        );
    }

    #[test]
    fn invalid_shapes_and_duplicate_tags_are_visible() {
        let xml = format!(
            "<rdf:RDF xmlns:rdf=\"{RDF}\" xmlns:dc=\"{DC}\" xmlns:xmp=\"{XMP}\" xmlns:xmpRights=\"{RIGHTS}\"><rdf:Description><dc:title><rdf:Alt><rdf:li xml:lang=\"EN-us\">A</rdf:li><rdf:li xml:lang=\"en-US\">B</rdf:li></rdf:Alt></dc:title><dc:subject><rdf:Seq><rdf:li>a</rdf:li></rdf:Seq></dc:subject><dc:creator><rdf:Bag/></dc:creator><xmp:Rating/><xmpRights:Marked/></rdf:Description></rdf:RDF>"
        );
        let d = XmpDocument::parse(xml.as_bytes()).unwrap();
        assert_eq!(d.title(), FieldState::Invalid);
        assert_eq!(d.keywords(), FieldState::Invalid);
        assert_eq!(d.creators(), FieldState::Invalid);
        assert_eq!(d.rating(), FieldState::Invalid);
        assert_eq!(d.marked(), FieldState::Invalid);
    }

    #[test]
    fn rejects_unpreservable_constructs_and_malformed_xml() {
        for attr in [
            "rdf:ID=\"id\"",
            "rdf:nodeID=\"id\"",
            "rdf:bagID=\"id\"",
            "rdf:about=\"#xpointer(id('x'))\"",
            "rdf:parseType=\"Literal\"",
        ] {
            let xml = format!("<rdf:RDF xmlns:rdf=\"{RDF}\"><rdf:Description {attr}/></rdf:RDF>");
            assert_eq!(
                XmpDocument::parse(xml.as_bytes()).unwrap_err(),
                XmpParseError::Unpreservable,
                "{attr}"
            );
        }
        assert_eq!(
            XmpDocument::parse(
                format!("<!DOCTYPE rdf:RDF [<!ENTITY x 'bad'>]><rdf:RDF xmlns:rdf=\"{RDF}\"/>")
                    .as_bytes()
            )
            .unwrap_err(),
            XmpParseError::Unpreservable
        );
        for xml in [
            "<bad>",
            "<bad></different>",
            "<bad/><other/>",
            "<rdf:RDF xmlns:rdf='oops'><x></rdf:RDF>",
        ] {
            assert_eq!(
                XmpDocument::parse(xml.as_bytes()).unwrap_err(),
                XmpParseError::Malformed,
                "{xml}"
            );
        }
        assert_eq!(
            XmpDocument::parse(b"\xff").unwrap_err(),
            XmpParseError::Malformed
        );
        assert_eq!(
            XmpDocument::parse(format!("<rdf:RDF xmlns:rdf=\"{RDF}\">\u{1}</rdf:RDF>").as_bytes())
                .unwrap_err(),
            XmpParseError::Malformed
        );
        for construct in ["rdf:parseType=\"Collection\"", "rdf:parseType=\"Other\""] {
            let xml = format!(
                "<r:RDF xmlns:r=\"{RDF}\" xmlns:rdf=\"{RDF}\"><r:Description {construct}/></r:RDF>"
            );
            assert_eq!(
                XmpDocument::parse(xml.as_bytes()).unwrap_err(),
                XmpParseError::Unpreservable
            );
        }
    }

    #[test]
    fn all_parser_limits_are_enforced() {
        let packet = vec![b' '; MAXIMUM_XMP_PACKET_BYTES as usize + 1];
        assert_eq!(
            XmpDocument::parse(&packet).unwrap_err(),
            XmpParseError::ResourceLimit
        );
        let huge = "a".repeat(MAX_TEXT + 1);
        assert_eq!(XmpDocument::parse(format!("<rdf:RDF xmlns:rdf=\"{RDF}\"><rdf:Description><a xmlns=\"urn:a\">{huge}</a></rdf:Description></rdf:RDF>").as_bytes()).unwrap_err(), XmpParseError::ResourceLimit);
        let split = format!(
            "<rdf:RDF xmlns:rdf=\"{RDF}\"><rdf:Description><a xmlns=\"urn:a\">{}&amp;{}</a></rdf:Description></rdf:RDF>",
            "a".repeat(MAX_TEXT / 2),
            "b".repeat(MAX_TEXT / 2)
        );
        assert_eq!(
            XmpDocument::parse(split.as_bytes()).unwrap_err(),
            XmpParseError::ResourceLimit
        );
        let split = format!(
            "<rdf:RDF xmlns:rdf=\"{RDF}\"><rdf:Description><a xmlns=\"urn:a\">{}<![CDATA[{}]]></a></rdf:Description></rdf:RDF>",
            "a".repeat(MAX_TEXT / 2),
            "b".repeat(MAX_TEXT / 2 + 1)
        );
        assert_eq!(
            XmpDocument::parse(split.as_bytes()).unwrap_err(),
            XmpParseError::ResourceLimit
        );
        assert_eq!(
            XmpDocument::parse(
                format!("<rdf:RDF xmlns:rdf=\"{RDF}\" rdf:about=\"{huge}\"/>").as_bytes()
            )
            .unwrap_err(),
            XmpParseError::ResourceLimit
        );
        let mut deep = format!("<rdf:RDF xmlns:rdf=\"{RDF}\" xmlns:a=\"urn:a\">");
        for _ in 0..MAX_DEPTH {
            deep.push_str("<a:a>");
        }
        for _ in 0..MAX_DEPTH {
            deep.push_str("</a:a>");
        }
        deep.push_str("</rdf:RDF>");
        assert_eq!(
            XmpDocument::parse(deep.as_bytes()).unwrap_err(),
            XmpParseError::ResourceLimit
        );
        let nodes = format!(
            "<rdf:RDF xmlns:rdf=\"{RDF}\">{}</rdf:RDF>",
            "<rdf:Description/>".repeat(MAX_NODES)
        );
        assert_eq!(
            XmpDocument::parse(nodes.as_bytes()).unwrap_err(),
            XmpParseError::ResourceLimit
        );
        let items = format!(
            "<rdf:RDF xmlns:rdf=\"{RDF}\"><rdf:Description><rdf:Bag>{}</rdf:Bag></rdf:Description></rdf:RDF>",
            "<rdf:li/>".repeat(MAX_ITEMS + 1)
        );
        assert_eq!(
            XmpDocument::parse(items.as_bytes()).unwrap_err(),
            XmpParseError::ResourceLimit
        );
        let languages = format!(
            "<rdf:RDF xmlns:rdf=\"{RDF}\"><rdf:Description><rdf:Alt>{}</rdf:Alt></rdf:Description></rdf:RDF>",
            "<rdf:li xml:lang=\"en\"/>".repeat(MAX_LANGUAGES + 1)
        );
        assert_eq!(
            XmpDocument::parse(languages.as_bytes()).unwrap_err(),
            XmpParseError::ResourceLimit
        );
    }

    #[test]
    fn captures_only_present_sidecar_representations() {
        let d = doc();
        assert_eq!(
            d.capture_representations(),
            vec![
                ("exif:DateTimeOriginal", "2020:01:02 03:04:05".into()),
                ("tiff:Make", "Camera".into())
            ]
        );
        let all = format!(
            "<rdf:RDF xmlns:rdf=\"{RDF}\" xmlns:exif=\"{EXIF}\" xmlns:tiff=\"{TIFF}\"><rdf:Description exif:PixelXDimension=\"1200\" exif:PixelYDimension=\"800\" tiff:Orientation=\"1\" exif:LensModel=\"Glass\" tiff:Model=\"Model\"/></rdf:RDF>"
        );
        assert_eq!(
            XmpDocument::parse(all.as_bytes())
                .unwrap()
                .capture_representations()
                .len(),
            5
        );
    }
}
