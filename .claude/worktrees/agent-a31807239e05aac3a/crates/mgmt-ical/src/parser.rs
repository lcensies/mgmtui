//! Low-level iCalendar parsing: line unfolding and a flat component/property tree.
//!
//! iCalendar content is a nested set of `BEGIN:X` / `END:X` blocks, each holding
//! `NAME;PARAM=val:VALUE` property lines. We parse into a generic [`Component`] tree and
//! let the typed mappers (`vevent`, `vtodo`) interpret it.

use mgmt_core::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prop {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub value: String,
}

impl Prop {
    pub fn param(&self, key: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    pub name: String,
    pub props: Vec<Prop>,
    pub children: Vec<Component>,
}

impl Component {
    pub fn prop(&self, name: &str) -> Option<&Prop> {
        self.props.iter().find(|p| p.name.eq_ignore_ascii_case(name))
    }

    pub fn value(&self, name: &str) -> Option<&str> {
        self.prop(name).map(|p| p.value.as_str())
    }

    pub fn child(&self, name: &str) -> Option<&Component> {
        self.children.iter().find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// Depth-first search for the first descendant component named `name`.
    pub fn find(&self, name: &str) -> Option<&Component> {
        if self.name.eq_ignore_ascii_case(name) {
            return Some(self);
        }
        self.children.iter().find_map(|c| c.find(name))
    }
}

/// Undo RFC 5545 line folding: a CRLF (or LF) followed by a space or tab is a continuation.
fn unfold(input: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in input.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.is_empty() {
            continue;
        }
        if (line.starts_with(' ') || line.starts_with('\t')) && !lines.is_empty() {
            let cont = &line[1..];
            lines.last_mut().unwrap().push_str(cont);
        } else {
            lines.push(line.to_string());
        }
    }
    lines
}

fn parse_prop(line: &str) -> Result<Prop> {
    // Split name+params from value at the first unquoted colon.
    let mut in_quote = false;
    let mut colon = None;
    for (i, ch) in line.char_indices() {
        match ch {
            '"' => in_quote = !in_quote,
            ':' if !in_quote => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon.ok_or_else(|| Error::Parse(format!("property without ':': {line}")))?;
    let (head, value) = line.split_at(colon);
    let value = value[1..].to_string();

    let mut parts = head.split(';');
    let name = parts
        .next()
        .ok_or_else(|| Error::Parse("empty property name".into()))?
        .to_string();
    let mut params = Vec::new();
    for p in parts {
        if let Some((k, v)) = p.split_once('=') {
            params.push((k.to_string(), v.trim_matches('"').to_string()));
        }
    }
    Ok(Prop { name, params, value })
}

/// Parse iCalendar text into the top-level component (typically `VCALENDAR`). If multiple
/// top-level components exist, they are wrapped in a synthetic `ROOT` component.
pub fn parse(input: &str) -> Result<Component> {
    let lines = unfold(input);
    let mut stack: Vec<Component> = Vec::new();
    let mut roots: Vec<Component> = Vec::new();

    for line in lines {
        if let Some(name) = line.strip_prefix("BEGIN:") {
            stack.push(Component {
                name: name.trim().to_string(),
                props: Vec::new(),
                children: Vec::new(),
            });
        } else if let Some(name) = line.strip_prefix("END:") {
            let comp = stack
                .pop()
                .ok_or_else(|| Error::Parse(format!("END:{name} without BEGIN")))?;
            if comp.name != name.trim() {
                return Err(Error::Parse(format!(
                    "mismatched END: expected {}, got {}",
                    comp.name,
                    name.trim()
                )));
            }
            match stack.last_mut() {
                Some(parent) => parent.children.push(comp),
                None => roots.push(comp),
            }
        } else {
            let prop = parse_prop(&line)?;
            match stack.last_mut() {
                Some(comp) => comp.props.push(prop),
                None => {} // property outside any component — ignore
            }
        }
    }

    if !stack.is_empty() {
        return Err(Error::Parse("unterminated component".into()));
    }
    match roots.len() {
        0 => Err(Error::Parse("no components found".into())),
        1 => Ok(roots.pop().unwrap()),
        _ => Ok(Component {
            name: "ROOT".to_string(),
            props: Vec::new(),
            children: roots,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_nested_components() {
        let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:1\r\nSUMMARY:hi\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
        let c = parse(ics).unwrap();
        assert_eq!(c.name, "VCALENDAR");
        let e = c.child("VEVENT").unwrap();
        assert_eq!(e.value("UID"), Some("1"));
        assert_eq!(e.value("SUMMARY"), Some("hi"));
    }

    #[test]
    fn unfolds_continuation_lines() {
        let ics = "BEGIN:VEVENT\r\nDESCRIPTION:hello \r\n world\r\nEND:VEVENT\r\n";
        let c = parse(ics).unwrap();
        assert_eq!(c.value("DESCRIPTION"), Some("hello world"));
    }

    #[test]
    fn parses_params() {
        let ics = "BEGIN:VEVENT\r\nDTSTART;VALUE=DATE:20260618\r\nEND:VEVENT\r\n";
        let c = parse(ics).unwrap();
        let p = c.prop("DTSTART").unwrap();
        assert_eq!(p.param("VALUE"), Some("DATE"));
        assert_eq!(p.value, "20260618");
    }

    #[test]
    fn rejects_mismatched_end() {
        assert!(parse("BEGIN:VEVENT\r\nEND:VTODO\r\n").is_err());
    }
}
