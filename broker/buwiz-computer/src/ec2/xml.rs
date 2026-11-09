//! The EC2 Query API answers in XML: a small tree of it, read with `quick-xml`, with a limit on
//! how deep and how big. Namespaces are dropped; EC2's names don't clash.

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::{Problem, Result};

const MAX_DEPTH: usize = 48;
const MAX_NODES: usize = 200_000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Node {
    pub name: String,
    pub text: String,
    pub children: Vec<Node>,
}

fn bad(why: &str) -> Problem {
    Problem::Failed(format!("AWS's answer couldn't be read: {why}"))
}

/// The document's root element.
pub fn parse(xml: &str) -> Result<Node> {
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<Node> = vec![Node::default()];
    let mut count = 0;
    loop {
        let event = reader.read_event().map_err(|_| bad("it isn't XML"))?;
        match event {
            Event::Start(e) | Event::Empty(e) if stack.len() > MAX_DEPTH => return Err(bad(&format!("deeper than {MAX_DEPTH} at {}", String::from_utf8_lossy(e.local_name().as_ref())))),
            Event::Start(e) => {
                count += 1;
                stack.push(Node { name: String::from_utf8_lossy(e.local_name().as_ref()).into_owned(), ..Node::default() });
            }
            Event::Empty(e) => {
                count += 1;
                let node = Node { name: String::from_utf8_lossy(e.local_name().as_ref()).into_owned(), ..Node::default() };
                stack.last_mut().ok_or_else(|| bad("no parent"))?.children.push(node);
            }
            Event::End(_) => {
                let node = stack.pop().ok_or_else(|| bad("an end with no start"))?;
                stack.last_mut().ok_or_else(|| bad("an end with no start"))?.children.push(node);
            }
            Event::Text(t) => {
                let text = t.decode().map_err(|_| bad("text that isn't UTF-8"))?;
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&text);
                }
            }
            Event::CData(t) => {
                if let Some(top) = stack.last_mut() {
                    top.text.push_str(&String::from_utf8_lossy(&t));
                }
            }
            Event::GeneralRef(r) => {
                let ch = match r.resolve_char_ref().map_err(|_| bad("a bad character reference"))? {
                    Some(c) => c,
                    None => match r.as_ref() {
                        b"lt" => '<',
                        b"gt" => '>',
                        b"amp" => '&',
                        b"apos" => '\'',
                        b"quot" => '"',
                        _ => return Err(bad("an unknown entity")),
                    },
                };
                if let Some(top) = stack.last_mut() {
                    top.text.push(ch);
                }
            }
            Event::Eof => break,
            _ => {}
        }
        if count > MAX_NODES {
            return Err(bad("too big"));
        }
    }
    if stack.len() != 1 {
        return Err(bad("it ends early"));
    }
    stack.pop().and_then(|doc| doc.children.into_iter().next()).ok_or_else(|| bad("it's empty"))
}

impl Node {
    pub fn child(&self, name: &str) -> Option<&Node> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn at(&self, path: &[&str]) -> Option<&Node> {
        path.iter().try_fold(self, |node, name| node.child(name))
    }

    /// The trimmed text at `path`, when there is some.
    pub fn text_at(&self, path: &[&str]) -> Option<&str> {
        self.at(path).map(|n| n.text.trim()).filter(|t| !t.is_empty())
    }

    /// The `<item>`s of the list at `path`: EC2's every list.
    pub fn items<'a>(&'a self, path: &[&str]) -> impl Iterator<Item = &'a Node> + 'a {
        self.at(path).into_iter().flat_map(|set| set.children.iter().filter(|c| c.name == "item"))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_with_entities_and_namespaces() {
        let doc = parse(r#"<?xml version="1.0" encoding="UTF-8"?><R xmlns="http://ec2.amazonaws.com/doc/2016-11-15/"><a>x &amp; &#x41;</a><s><item><v>1</v></item><item><v>2</v></item><other/></s></R>"#).unwrap();
        assert_eq!(doc.name, "R");
        assert_eq!(doc.text_at(&["a"]), Some("x & A"));
        assert_eq!(doc.items(&["s"]).map(|i| i.text_at(&["v"]).unwrap()).collect::<Vec<_>>(), ["1", "2"]);
        assert_eq!(doc.items(&["nothing"]).count(), 0);
    }

    #[test]
    fn what_isnt_whole_is_refused() {
        for bad in ["", "<a><b></a>", "<a>", "<a>&nope;</a>"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        let deep = "<a>".repeat(100) + &"</a>".repeat(100);
        assert!(parse(&deep).is_err());
    }
}
